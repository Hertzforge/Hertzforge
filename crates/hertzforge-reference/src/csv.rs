//! A plain CSV layout for a reference: a PSD table and an optional pair table.
//!
//! **PSD table.** A header row `frequency_hz` then one column per control, named by
//! the control's label; then one row per line, the frequency in hertz and each
//! control's PSD in the control unit squared per hertz.
//!
//! ```text
//! frequency_hz,front,rear
//! 20,0.01,0.02
//! 21,0.01,0.02
//! ```
//!
//! **Pair table.** A header row `frequency_hz,row,column,coherence,phase_deg`, then
//! one row per pair per line: the frequency, the two controls by label, the
//! squared coherence (0 to 1) and the phase of `G_row,column` in degrees, in this
//! controller's convention `G = E[X X^H]` (a positive phase means the column's
//! control lags the row's). No conjugation is applied: this is the controller's
//! own layout. A pair not listed on a line takes the import's
//! [`UnspecifiedPairs`] choice with zero phase.
//!
//! Both tables: comma separated (tab separated when the header has tabs and no
//! commas); no quoting, so labels hold no commas; blank lines and lines starting
//! with `#` are skipped. Frequencies are placed on the grid by the same rule as the
//! `.npz` import ([`crate::npz::match_lines`]); grid lines not listed are zero.
//! Alarm and abort lines are the import's dB bands about each PSD.

use crate::builders::{Grid, UnspecifiedPairs, DEFAULT_ABORT_DB, DEFAULT_ALARM_DB};
use crate::npz::match_lines;
use crate::{
    band_of, Method, Provenance, PsdRepairReport, Reference, ReferenceError, ReferenceParts,
    RepairPolicy, Tolerances, C64,
};
use hertzforge_sdm::HermitianStack;

/// How a CSV reference is read.
#[derive(Clone, Debug, PartialEq)]
pub struct CsvImport {
    pub grid: Grid,
    pub unit: String,
    pub unspecified: UnspecifiedPairs,
    /// Alarm band (below, above) in dB about each PSD.
    pub alarm_db: (f64, f64),
    /// Abort band (below, above) in dB about each PSD.
    pub abort_db: (f64, f64),
    pub repair: Option<RepairPolicy>,
}

impl CsvImport {
    /// Independent pairs, the default bands, unit `g`, no repair.
    pub fn new(grid: Grid) -> Self {
        CsvImport {
            grid,
            unit: "g".into(),
            unspecified: UnspecifiedPairs::Independent,
            alarm_db: DEFAULT_ALARM_DB,
            abort_db: DEFAULT_ABORT_DB,
            repair: None,
        }
    }
}

/// The rows of a table: (1-based source line number, cells).
fn rows(text: &str) -> Vec<(usize, Vec<String>)> {
    let mut delim = ',';
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if out.is_empty() && t.contains('\t') && !t.contains(',') {
            delim = '\t';
        }
        out.push((
            i + 1,
            t.split(delim).map(|c| c.trim().to_string()).collect(),
        ));
    }
    out
}

fn number(cell: &str, what: &str, line: usize, table: &str) -> Result<f64, ReferenceError> {
    cell.parse::<f64>().map_err(|_| {
        ReferenceError::File(format!(
            "{table}, line {line}: the {what} {cell:?} is not a number"
        ))
    })
}

/// Read a reference from a PSD table and an optional pair table.
pub fn import(
    psd_csv: &str,
    pair_csv: Option<&str>,
    source: &str,
    opts: &CsvImport,
) -> Result<(Reference, Option<PsdRepairReport>), ReferenceError> {
    let grid = Grid::new(opts.grid.df_hz, opts.grid.lines)?;
    let table = "the PSD table";
    let mut psd_rows = rows(psd_csv).into_iter();
    let (hline, header) = psd_rows.next().ok_or_else(|| {
        ReferenceError::File(format!(
            "{table} is empty; give a header and one row per line"
        ))
    })?;
    if header.first().map(String::as_str) != Some("frequency_hz") || header.len() < 2 {
        return Err(ReferenceError::File(format!(
            "{table}, line {hline}: the header must be frequency_hz followed by one column per \
             control"
        )));
    }
    let labels: Vec<String> = header[1..].to_vec();
    let n = labels.len();
    let mut freqs = Vec::new();
    let mut levels = Vec::new();
    for (line, cells) in psd_rows {
        if cells.len() != n + 1 {
            return Err(ReferenceError::File(format!(
                "{table}, line {line}: {} cells where the header has {}",
                cells.len(),
                n + 1
            )));
        }
        freqs.push(number(&cells[0], "frequency", line, table)?);
        for (k, c) in cells[1..].iter().enumerate() {
            let v = number(c, &format!("PSD of {}", labels[k]), line, table)?;
            if !(v.is_finite() && v >= 0.0) {
                return Err(ReferenceError::File(format!(
                    "{table}, line {line}: the PSD of {} is {v}; a PSD is a number at or above 0",
                    labels[k]
                )));
            }
            levels.push(v);
        }
    }
    let at = match_lines(&freqs, grid, table)?;
    let mut sdm = HermitianStack::zeros(grid.lines, n);
    for (idx, &k) in at.iter().enumerate() {
        for i in 0..n {
            sdm.set(k, i, i, C64::new(levels[idx * n + i], 0.0));
        }
    }
    let default = match opts.unspecified {
        UnspecifiedPairs::Independent => 0.0,
        UnspecifiedPairs::FullyCoherent => 1.0,
    };
    // Per line and pair (i < j): Some((coherence, phase of G_ij)).
    let mut given: Vec<Option<(f64, f64)>> = vec![None; grid.lines * n * n];
    if let Some(text) = pair_csv {
        let table = "the pair table";
        let mut it = rows(text).into_iter();
        let (hline, header) = it.next().ok_or_else(|| {
            ReferenceError::File(format!("{table} is empty; give a header or leave it out"))
        })?;
        let want = ["frequency_hz", "row", "column", "coherence", "phase_deg"];
        if header != want {
            return Err(ReferenceError::File(format!(
                "{table}, line {hline}: the header must be {}",
                want.join(",")
            )));
        }
        let index = |label: &str, line: usize| {
            labels.iter().position(|l| l == label).ok_or_else(|| {
                ReferenceError::File(format!(
                    "{table}, line {line}: {label:?} is not a control in the PSD table ({})",
                    labels.join(", ")
                ))
            })
        };
        for (line, cells) in it {
            if cells.len() != 5 {
                return Err(ReferenceError::File(format!(
                    "{table}, line {line}: {} cells where the header has 5",
                    cells.len()
                )));
            }
            let f = number(&cells[0], "frequency", line, table)?;
            let k = match_lines(&[f], grid, table)?[0];
            if !at.contains(&k) {
                return Err(ReferenceError::File(format!(
                    "{table}, line {line}: {f} Hz has no row in the PSD table; a pair needs both \
                     PSDs on its line"
                )));
            }
            let (r, c) = (index(&cells[1], line)?, index(&cells[2], line)?);
            if r == c {
                return Err(ReferenceError::File(format!(
                    "{table}, line {line}: row and column are both {}; a pair is two controls",
                    cells[1]
                )));
            }
            let coh = number(&cells[3], "coherence", line, table)?;
            if !(0.0..=1.0).contains(&coh) {
                return Err(ReferenceError::File(format!(
                    "{table}, line {line}: coherence {coh} is outside 0 to 1 (squared coherence)"
                )));
            }
            let ph = number(&cells[4], "phase", line, table)?;
            if !ph.is_finite() {
                return Err(ReferenceError::File(format!(
                    "{table}, line {line}: the phase is not a finite number"
                )));
            }
            let (i, j, ph) = if r < c { (r, c, ph) } else { (c, r, -ph) };
            let slot = &mut given[(k * n + i) * n + j];
            if slot.is_some() {
                return Err(ReferenceError::File(format!(
                    "{table}, line {line}: the pair {}, {} at {f} Hz is given twice",
                    labels[i], labels[j]
                )));
            }
            *slot = Some((coh, ph));
        }
    }
    for k in 0..grid.lines {
        let d = sdm.diagonal(k);
        for i in 0..n {
            for j in i + 1..n {
                if d[i] <= 0.0 || d[j] <= 0.0 {
                    continue;
                }
                let (coh, ph) = given[(k * n + i) * n + j].unwrap_or((default, 0.0));
                let mag = (d[i] * d[j] * coh).sqrt();
                sdm.set(k, i, j, C64::from_polar(mag, ph.to_radians()));
            }
        }
    }
    let band = band_of(&sdm)
        .ok_or_else(|| ReferenceError::File("the PSD table has no level on any line".into()))?;
    let mut tolerances = Tolerances::none(grid.lines, n);
    let db = |x: f64| 10f64.powf(x / 10.0);
    for k in band.0..band.1 {
        for i in 0..n {
            let g = sdm.get(k, i, i).re;
            let x = k * n + i;
            tolerances.alarm_upper[x] = g * db(opts.alarm_db.1);
            tolerances.alarm_lower[x] = g * db(-opts.alarm_db.0);
            tolerances.abort_upper[x] = g * db(opts.abort_db.1);
            tolerances.abort_lower[x] = g * db(-opts.abort_db.0);
        }
    }
    let mut provenance = Provenance::new(Method::Csv, source);
    let mut hashed = psd_csv.as_bytes().to_vec();
    if let Some(p) = pair_csv {
        hashed.extend_from_slice(p.as_bytes());
    }
    provenance.source_sha256 = Some(crate::native::content_hash(&hashed));
    provenance.assumptions = vec![
        "phases in the controller's convention, E[X X^H]; not conjugated".to_string(),
        format!(
            "alarm {:?} dB and abort {:?} dB (below, above) about each PSD",
            opts.alarm_db, opts.abort_db
        ),
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
mod tests {
    use super::*;

    const PSD: &str = "# a two-control reference\nfrequency_hz,front,rear\n20,0.01,0.04\n\
                       21,0.01,0.04\n22,0.02,0.04\n";

    #[test]
    fn a_psd_and_pair_table_read_into_a_reference() {
        let pairs = "frequency_hz,row,column,coherence,phase_deg\n21,front,rear,0.5,30\n\
                     22,rear,front,0.25,30\n";
        let (r, _) = import(
            PSD,
            Some(pairs),
            "t.csv",
            &CsvImport::new(Grid::new(1.0, 32).unwrap()),
        )
        .unwrap();
        assert_eq!(r.band, (20, 23));
        assert_eq!(r.labels, vec!["front", "rear"]);
        assert_eq!(r.sdm.get(22, 0, 0).re, 0.02);
        assert_eq!(r.sdm.get(20, 0, 1), C64::new(0.0, 0.0));
        let g = r.sdm.get(21, 0, 1);
        assert!((g.norm_sqr() / (0.01 * 0.04) - 0.5).abs() < 1e-12);
        assert!((g.arg().to_degrees() - 30.0).abs() < 1e-9);
        // Given as (rear, front): the phase of G_front,rear is its negative.
        assert!((r.sdm.get(22, 0, 1).arg().to_degrees() + 30.0).abs() < 1e-9);
        assert!((r.tolerances.abort_upper[21 * 2 + 1] / 0.04 - 10f64.powf(0.6)).abs() < 1e-12);
        assert_eq!(r.provenance.method, Method::Csv);
    }

    #[test]
    fn tab_separated_tables_read_too() {
        let tsv = PSD.replace(',', "\t");
        let (r, _) = import(
            &tsv,
            None,
            "t.tsv",
            &CsvImport::new(Grid::new(1.0, 32).unwrap()),
        )
        .unwrap();
        assert_eq!(r.n(), 2);
    }

    #[test]
    fn bad_tables_are_refused_with_the_line() {
        let g = CsvImport::new(Grid::new(1.0, 32).unwrap());
        let e = import("freq,a\n1,2\n", None, "t", &g).unwrap_err();
        assert!(e.to_string().contains("frequency_hz"), "{e}");
        let e = import(
            PSD,
            Some("frequency_hz,row,column,coherence,phase_deg\n21,front,side,0.5,0\n"),
            "t",
            &g,
        )
        .unwrap_err();
        assert!(
            e.to_string().contains("line 2") && e.to_string().contains("\"side\""),
            "{e}"
        );
        let e = import(
            PSD,
            Some("frequency_hz,row,column,coherence,phase_deg\n25,front,rear,0.5,0\n"),
            "t",
            &g,
        )
        .unwrap_err();
        assert!(e.to_string().contains("no row in the PSD table"), "{e}");
        let e = import(&PSD.replace("21,", "21.5,"), None, "t", &g).unwrap_err();
        assert!(e.to_string().contains("21.5 Hz"), "{e}");
        let e = import(&PSD.replace("0.02", "x"), None, "t", &g).unwrap_err();
        assert!(e.to_string().contains("line 5"), "{e}");
    }
}
