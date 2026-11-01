//! CSV-minimal reader/writer with a PREVIEW-AUTHORITATIVE flow (a deliberately
//! minimal scope).
//!
//! v1 deliberately does NOT infer FRF triplets or axis kind - detection stops
//! at delimiter / decimal-comma / header+units rows, and everything else is
//! confirmed by the user in the preview sheet (`CsvOptions` is what the sheet
//! produces). A full smart-sniffer (encoding zoo, joint
//! delimiter-decimal inference, column-role classification) is a later wave.

use crate::{ImportError, ImportResult};
use eigen_data::{
    units, Abscissa, FnId, FunctionKind, FunctionRecord, Ordinate, Provenance, SourceRef, Unit,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Delimiter {
    Comma,
    Semicolon,
    Tab,
    Whitespace,
}

impl Delimiter {
    fn split<'a>(&self, line: &'a str) -> Vec<&'a str> {
        match self {
            Delimiter::Comma => line.split(',').collect(),
            Delimiter::Semicolon => line.split(';').collect(),
            Delimiter::Tab => line.split('\t').collect(),
            Delimiter::Whitespace => line.split_whitespace().collect(),
        }
    }
}

/// What the preview sheet shows and the user can override before commit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CsvPreview {
    pub delimiter: Delimiter,
    pub decimal_comma: bool,
    /// Header row (column names) when detected.
    pub headers: Option<Vec<String>>,
    /// Units row (per-column unit labels) when detected under the header.
    pub units_row: Option<Vec<String>>,
    pub n_columns: usize,
    pub n_data_rows: usize,
    /// First few parsed data rows for the preview table.
    pub sample_rows: Vec<Vec<f64>>,
    /// True when column 0 is strictly increasing (a plausible axis) - shown as
    /// a HINT; the user confirms the axis meaning, never the sniffer.
    pub first_column_increasing: bool,
}

/// The committed decisions (the preview sheet's output).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CsvOptions {
    pub delimiter: Delimiter,
    pub decimal_comma: bool,
    /// Rows to skip before data (header block incl. header/units rows).
    pub skip_rows: usize,
    /// Column names to use (preview-confirmed; positional).
    pub names: Vec<String>,
    /// Unit label per DATA column (parallel to names minus the axis column).
    pub unit_labels: Vec<String>,
    /// Column 0 semantics: the abscissa unit label ("s" or "Hz" typically).
    pub axis_unit: String,
    /// The kind assigned to every data column (one file, one kind - v1).
    pub kind: FunctionKind,
}

fn normalize_number(field: &str, decimal_comma: bool) -> Option<f64> {
    let mut t = field.trim().to_string();
    if t.is_empty() {
        return None;
    }
    if decimal_comma {
        t = t.replace(',', ".");
    }
    let t = t.replace(['D', 'd'], "E");
    t.parse::<f64>().ok()
}

fn looks_numeric_row(fields: &[&str], decimal_comma: bool) -> bool {
    let mut n = 0usize;
    for f in fields {
        if f.trim().is_empty() {
            continue;
        }
        if normalize_number(f, decimal_comma).is_none() {
            return false;
        }
        n += 1;
    }
    n >= 1
}

/// Detect delimiter + decimal-comma jointly on a text sample: score each
/// delimiter by how many consistent multi-column numeric rows it yields
/// (decimal-comma tried when the delimiter is not a comma).
fn detect(lines: &[&str]) -> (Delimiter, bool) {
    let mut best = (Delimiter::Comma, false, 0usize, 0usize);
    for d in [
        Delimiter::Comma,
        Delimiter::Semicolon,
        Delimiter::Tab,
        Delimiter::Whitespace,
    ] {
        for dc in [false, true] {
            if dc && d == Delimiter::Comma {
                continue; // decimal comma cannot coexist with comma delimiter
            }
            let mut rows = 0usize;
            let mut cols = 0usize;
            for l in lines.iter().take(50) {
                let f = d.split(l);
                if f.len() >= 2 && looks_numeric_row(&f, dc) {
                    rows += 1;
                    cols = cols.max(f.len());
                }
            }
            if (rows, cols) > (best.2, best.3) {
                best = (d, dc, rows, cols);
            }
        }
    }
    (best.0, best.1)
}

/// A units row is short bracketed/parenthesized or bare unit tokens.
fn looks_units_row(fields: &[&str]) -> bool {
    let mut hits = 0usize;
    let mut total = 0usize;
    for f in fields {
        let t = f.trim().trim_matches(['[', ']', '(', ')']);
        if t.is_empty() {
            continue;
        }
        total += 1;
        if t.len() <= 8
            && (units::unit(t).is_some()
                || t.eq_ignore_ascii_case("hz")
                || t.eq_ignore_ascii_case("s"))
        {
            hits += 1;
        }
    }
    total > 0 && hits * 2 >= total
}

/// Build the preview: detection + header/units-row identification + samples.
pub fn preview(bytes: &[u8]) -> Result<CsvPreview, ImportError> {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return Err(ImportError::Malformed {
            detail: "file is empty".into(),
            offset: None,
        });
    }
    let (delimiter, decimal_comma) = detect(&lines);
    // First numeric row index.
    let mut first_data = None;
    for (i, l) in lines.iter().enumerate() {
        let f = delimiter.split(l);
        if !f.is_empty() && looks_numeric_row(&f, decimal_comma) && f.len() >= 2 {
            first_data = Some(i);
            break;
        }
    }
    let first_data = first_data.ok_or_else(|| ImportError::Malformed {
        detail: "no numeric data rows found".into(),
        offset: None,
    })?;
    // Header = last non-numeric row before data; units row = the one between
    // header and data when it smells like units.
    let mut headers = None;
    let mut units_row = None;
    if first_data >= 1 {
        let h = delimiter.split(lines[first_data - 1]);
        if looks_units_row(&h) && first_data >= 2 {
            units_row = Some(h.iter().map(|s| s.trim().to_string()).collect());
            headers = Some(
                delimiter
                    .split(lines[first_data - 2])
                    .iter()
                    .map(|s| s.trim().to_string())
                    .collect(),
            );
        } else {
            headers = Some(h.iter().map(|s| s.trim().to_string()).collect());
        }
    }
    let n_columns = delimiter.split(lines[first_data]).len();
    let mut sample_rows = Vec::new();
    let mut prev0 = f64::NEG_INFINITY;
    let mut increasing = true;
    let mut n_data_rows = 0usize;
    for l in &lines[first_data..] {
        let f = delimiter.split(l);
        if !looks_numeric_row(&f, decimal_comma) {
            continue;
        }
        n_data_rows += 1;
        let row: Vec<f64> = f
            .iter()
            .filter_map(|x| normalize_number(x, decimal_comma))
            .collect();
        if let Some(&x0) = row.first() {
            if x0 <= prev0 {
                increasing = false;
            }
            prev0 = x0;
        }
        if sample_rows.len() < 8 {
            sample_rows.push(row);
        }
    }
    Ok(CsvPreview {
        delimiter,
        decimal_comma,
        headers,
        units_row,
        n_columns,
        n_data_rows,
        sample_rows,
        first_column_increasing: increasing,
    })
}

/// Commit an import with preview-confirmed options.
pub fn import_with(
    bytes: &[u8],
    path: &str,
    opts: &CsvOptions,
) -> Result<ImportResult, ImportError> {
    let text = String::from_utf8_lossy(bytes);
    let mut warnings = Vec::new();
    let mut axis: Vec<f64> = Vec::new();
    let mut cols: Vec<Vec<f64>> = Vec::new();
    let mut n_cols_expected = 0usize;
    for (row_i, raw) in text.lines().enumerate() {
        if row_i < opts.skip_rows {
            continue;
        }
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let fields = opts.delimiter.split(line);
        if !looks_numeric_row(&fields, opts.decimal_comma) {
            warnings.push(format!("row {}: non-numeric, skipped", row_i + 1));
            continue;
        }
        let row: Vec<f64> = fields
            .iter()
            .filter_map(|f| normalize_number(f, opts.decimal_comma))
            .collect();
        if row.len() < 2 {
            warnings.push(format!("row {}: fewer than 2 columns, skipped", row_i + 1));
            continue;
        }
        if n_cols_expected == 0 {
            n_cols_expected = row.len();
            cols = vec![Vec::new(); n_cols_expected - 1];
        }
        if row.len() != n_cols_expected {
            warnings.push(format!(
                "row {}: {} columns (expected {n_cols_expected}), skipped",
                row_i + 1,
                row.len()
            ));
            continue;
        }
        axis.push(row[0]);
        for (c, v) in row[1..].iter().enumerate() {
            cols[c].push(*v);
        }
    }
    if axis.len() < 2 {
        return Err(ImportError::Malformed {
            detail: "fewer than 2 usable data rows".into(),
            offset: None,
        });
    }
    if axis.windows(2).any(|w| w[1] <= w[0]) {
        return Err(ImportError::Malformed {
            detail:
                "axis column is not strictly increasing - confirm the axis column in the preview"
                    .into(),
            offset: None,
        });
    }
    let axis_unit = units::unit(&opts.axis_unit).unwrap_or_else(|| Unit::unknown(&opts.axis_unit));
    let stem = std::path::Path::new(path)
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("csv");
    let mut functions = Vec::new();
    for (c, series) in cols.into_iter().enumerate() {
        let name = opts
            .names
            .get(c + 1)
            .filter(|n| !n.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| format!("{stem} column {}", c + 2));
        let unit_label = opts.unit_labels.get(c).cloned().unwrap_or_default();
        let unit = if unit_label.trim().is_empty() {
            warnings.push(format!("'{name}': no unit assigned - dimensionless"));
            Unit::dimensionless()
        } else {
            units::unit(&unit_label).unwrap_or_else(|| Unit::unknown(&unit_label))
        };
        functions.push(FunctionRecord {
            id: FnId(c as u64 + 1),
            name,
            role: None,
            response: None,
            reference: None,
            kind: opts.kind,
            abscissa: Abscissa::Explicit(axis.clone()),
            ordinate: Ordinate::Real(series),
            unit_ordinate: unit,
            unit_abscissa: axis_unit.clone(),
            t0: None,
            provenance: Provenance::Imported {
                format: "csv".into(),
                path: path.into(),
                detail: format!("{:?} delimiter", opts.delimiter),
            },
            source: SourceRef {
                path: path.into(),
                format: "csv".into(),
                imported_at: 0,
            },
            comments: vec![],
        });
    }
    Ok(ImportResult {
        functions,
        warnings,
    })
}

/// STRICT writer: axis column + one column per record, on the FIRST record's
/// grid - callers align first (the compare-set exporter passes aligned sets by
/// construction). Provenance/method lines ride as `#` comments (CSV
/// carries the method block inline).
pub fn write_csv(records: &[&FunctionRecord], method_lines: &[String]) -> Result<String, String> {
    let first = records.first().ok_or("nothing to export")?;
    let axis = first.abscissa.values();
    for r in records {
        if r.abscissa.values() != axis {
            return Err(format!(
                "'{}' is on a different grid - align before exporting (compare sets export aligned)",
                r.name
            ));
        }
        if r.ordinate.is_complex() {
            return Err(format!(
                "'{}' is complex - export magnitude/phase traces instead (v1 CSV is real-valued)",
                r.name
            ));
        }
    }
    let mut out = String::new();
    for m in method_lines {
        out.push_str("# ");
        out.push_str(m);
        out.push('\n');
    }
    for r in records {
        out.push_str(&format!("# {}: {}\n", r.name, r.provenance.recipe_line()));
    }
    // Header: names; units row beneath.
    out.push_str(&axis_header(first));
    for r in records {
        out.push(',');
        out.push_str(&r.name.replace(',', ";"));
    }
    out.push('\n');
    out.push_str(&format!("[{}]", first.unit_abscissa.label));
    for r in records {
        out.push(',');
        out.push_str(&format!("[{}]", r.unit_ordinate.label));
    }
    out.push('\n');
    let series: Vec<Vec<f64>> = records
        .iter()
        .map(|r| match &r.ordinate {
            Ordinate::Real(v) => v.clone(),
            Ordinate::Complex(_) => unreachable!(),
        })
        .collect();
    for (i, x) in axis.iter().enumerate() {
        out.push_str(&format!("{x:.9e}"));
        for s in &series {
            out.push_str(&format!(",{:.9e}", s.get(i).copied().unwrap_or(f64::NAN)));
        }
        out.push('\n');
    }
    Ok(out)
}

fn axis_header(rec: &FunctionRecord) -> String {
    match rec.kind {
        FunctionKind::TimeHistory => "time".into(),
        _ => "frequency".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_eu_locale_semicolon_decimal_comma() {
        let text = "freq;ch a;ch b\nHz;g;g\n10,0;0,5;1,5\n20,0;0,75;1,25\n30,0;1,0;1,0\n";
        let p = preview(text.as_bytes()).unwrap();
        assert_eq!(p.delimiter, Delimiter::Semicolon);
        assert!(p.decimal_comma);
        assert_eq!(p.headers.as_deref().unwrap()[1], "ch a");
        assert_eq!(p.units_row.as_deref().unwrap()[1], "g");
        assert_eq!(p.n_data_rows, 3);
        assert!(p.first_column_increasing);
    }

    #[test]
    fn detects_plain_comma_and_tab() {
        let c = preview(b"0.0,1.0\n0.1,2.0\n").unwrap();
        assert_eq!(c.delimiter, Delimiter::Comma);
        assert!(!c.decimal_comma);
        let t = preview(b"h1\th2\n0.0\t1.0\n1.0\t2.0\n").unwrap();
        assert_eq!(t.delimiter, Delimiter::Tab);
    }

    #[test]
    fn import_with_options_builds_named_records() {
        let text = "f,left,right\nHz,g,g\n10,0.1,0.4\n20,0.2,0.3\n40,0.3,0.2\n";
        let opts = CsvOptions {
            delimiter: Delimiter::Comma,
            decimal_comma: false,
            skip_rows: 2,
            names: vec![
                "f".into(),
                "pump inlet 12X+".into(),
                "pump outlet 14X+".into(),
            ],
            unit_labels: vec!["g".into(), "g".into()],
            axis_unit: "Hz".into(),
            kind: FunctionKind::Psd,
        };
        let out = import_with(text.as_bytes(), "/x/run7.csv", &opts).unwrap();
        assert_eq!(out.functions.len(), 2);
        assert_eq!(out.functions[0].name, "pump inlet 12X+");
        assert_eq!(out.functions[0].kind, FunctionKind::Psd);
        assert_eq!(out.functions[1].ordinate.len(), 3);
    }

    #[test]
    fn non_increasing_axis_refuses_with_guidance() {
        let text = "1.0,2.0\n0.5,3.0\n";
        let opts = CsvOptions {
            delimiter: Delimiter::Comma,
            decimal_comma: false,
            skip_rows: 0,
            names: vec![],
            unit_labels: vec![],
            axis_unit: "s".into(),
            kind: FunctionKind::TimeHistory,
        };
        let e = import_with(text.as_bytes(), "x.csv", &opts).unwrap_err();
        match e {
            ImportError::Malformed { detail, .. } => assert!(detail.contains("preview")),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn writer_round_trips_through_reader() {
        use eigen_data::{units::unit, Provenance, SourceRef};
        let rec = FunctionRecord {
            id: FnId(1),
            name: "beam tip 5Z+".into(),
            role: None,
            response: None,
            reference: None,
            kind: FunctionKind::Psd,
            abscissa: Abscissa::Explicit(vec![10.0, 20.0, 40.0]),
            ordinate: Ordinate::Real(vec![0.01, 0.02, 0.005]),
            unit_ordinate: unit("g").unwrap(),
            unit_abscissa: unit("Hz").unwrap(),
            t0: None,
            provenance: Provenance::Imported {
                format: "t".into(),
                path: "m".into(),
                detail: String::new(),
            },
            source: SourceRef {
                path: "m".into(),
                format: "t".into(),
                imported_at: 0,
            },
            comments: vec![],
        };
        let text = write_csv(&[&rec], &["method: test".into()]).unwrap();
        assert!(text.starts_with("# method: test\n"));
        let p = preview(text.as_bytes()).unwrap();
        assert_eq!(p.n_data_rows, 3);
        let opts = CsvOptions {
            delimiter: p.delimiter,
            decimal_comma: p.decimal_comma,
            skip_rows: 4, // 2 comment lines + header + units
            names: p.headers.clone().unwrap_or_default(),
            unit_labels: vec!["g".into()],
            axis_unit: "Hz".into(),
            kind: FunctionKind::Psd,
        };
        let back = import_with(text.as_bytes(), "rt.csv", &opts).unwrap();
        assert_eq!(back.functions.len(), 1);
        let Ordinate::Real(v) = &back.functions[0].ordinate else {
            panic!()
        };
        assert!((v[2] - 0.005).abs() < 1e-12);
    }
}
