//! `srs.target.import` and `srs.table.import` (schema/PROTOCOL.md, "SRS Shock
//! Configuration"): a breakpoint target and a synthesis component table read from
//! files on the computer that runs the test, each returned as the configuration
//! takes it, or with items naming what is wrong and where.

use super::config::{WireComponents, WireTarget};
use crate::protocol::{num, MethodError};
use crate::table::{Item, Report};
use hertzforge_srs::service::config::EngineKind;
use hertzforge_srs::table::{DampedSineTable, WaveletTable};
use serde_json::{json, Value};
use std::path::Path;

/// The largest file either import reads, bytes.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Read `path` as text: an absolute path, a readable file, at most
/// [`MAX_FILE_BYTES`]. A relative path is `bad_params`; a file that cannot be read
/// is an item of `reason`.
fn read(path: &str, reason: &'static str) -> Result<Result<String, Item>, MethodError> {
    if path.trim().is_empty() {
        return Err(MethodError::bad_params(
            "path",
            "An import needs the path of its file.",
        ));
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(MethodError::bad_params(
            "path",
            "Give the file's full path on this computer.",
        ));
    }
    let unreadable = |why: String| {
        Item::field(
            reason,
            "path",
            format!(
                "{} could not be read ({why}). Check the path and that the file is readable.",
                p.display()
            ),
        )
    };
    match std::fs::metadata(p) {
        Err(e) => return Ok(Err(unreadable(e.to_string()))),
        Ok(m) if !m.is_file() => return Ok(Err(unreadable("it is not a file".into()))),
        Ok(m) if m.len() > MAX_FILE_BYTES => {
            return Ok(Err(unreadable(format!(
                "it is {} bytes, over the {} bytes an import reads",
                m.len(),
                MAX_FILE_BYTES
            ))))
        }
        Ok(_) => {}
    }
    Ok(std::fs::read_to_string(p).map_err(|e| unreadable(e.to_string())))
}

/// The data lines of a file: its lines without blanks and `#` or `//` comments,
/// each with its line number from 1.
fn data_lines(text: &str) -> Vec<(usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim_end_matches('\r')))
        .filter(|(_, l)| {
            let t = l.trim_start();
            !(t.is_empty() || t.starts_with('#') || t.starts_with("//"))
        })
        .collect()
}

fn cells(line: &str) -> Vec<&str> {
    let d = if line.contains('\t') {
        '\t'
    } else if line.contains(';') {
        ';'
    } else if line.contains(',') {
        ','
    } else {
        ' '
    };
    if d == ' ' {
        line.split_whitespace().collect()
    } else {
        line.split(d).map(str::trim).collect()
    }
}

fn number(cell: &str) -> Option<f64> {
    cell.trim()
        .trim_matches('"')
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
}

/// Parse a breakpoint target from `text`: two columns, frequency in Hz and SRS in
/// g; a header row allowed. Rows are counted from 0 among the data rows.
pub fn parse_target_text(text: &str) -> (Option<WireTarget>, Report) {
    let mut r = Report::default();
    let lines = data_lines(text);
    let mut points = Vec::new();
    let mut row = 0usize;
    for (k, (line_no, line)) in lines.iter().enumerate() {
        let c = cells(line);
        let pair = (c.len() >= 2)
            .then(|| number(c[0]).zip(number(c[1])))
            .flatten();
        match pair {
            None if k == 0 => continue,
            None => r.errors.push(Item::row(
                "target_file",
                row,
                format!(
                    "Line {line_no} is not a frequency in Hz and an SRS in g: \"{}\".",
                    line.trim()
                ),
            )),
            Some((f, g)) => {
                if !(f > 0.0 && g > 0.0) {
                    r.errors.push(Item::row(
                        "target_file",
                        row,
                        format!(
                            "Line {line_no} gives {} Hz and {} g; each is above 0.",
                            num(f),
                            num(g)
                        ),
                    ));
                } else if let Some((pf, _)) = points.last().copied() {
                    if f <= pf {
                        r.errors.push(Item::row(
                            "target_file",
                            row,
                            format!(
                                "Line {line_no} is at {} Hz, not above the {} Hz before it; \
                                 list the breakpoints from the lowest frequency up.",
                                num(f),
                                num(pf)
                            ),
                        ));
                    }
                }
                if c.len() > 2 && c[2..].iter().any(|x| !x.trim().is_empty()) {
                    r.warnings.push(Item::row(
                        "target_file",
                        row,
                        format!("Line {line_no} has more than two columns; the rest are ignored."),
                    ));
                }
                points.push((f, g));
            }
        }
        row += 1;
    }
    if r.errors.is_empty() && points.len() < 2 {
        r.errors.push(Item::field(
            "target_file",
            "path",
            format!(
                "The file has {} breakpoint{}; a target has at least two.",
                points.len(),
                if points.len() == 1 { "" } else { "s" }
            ),
        ));
    }
    if r.errors.is_empty() {
        (Some(WireTarget::Breakpoints { points }), r)
    } else {
        (None, r)
    }
}

/// Parse a component table for `engine` from `text`, by the controller's own
/// importer; a row it refuses is named.
pub fn parse_table_text(text: &str, engine: EngineKind) -> (Option<WireComponents>, Report) {
    let parse = |s: &str| -> Result<WireComponents, String> {
        match engine {
            EngineKind::Wavsyn => WaveletTable::paste_from_clipboard(s)
                .map(|t| WireComponents::Wavsyn(t.components))
                .map_err(|e| e.to_string()),
            EngineKind::Seds => DampedSineTable::paste_from_clipboard(s)
                .map(|t| WireComponents::Seds(t.components))
                .map_err(|e| e.to_string()),
        }
    };
    let mut r = Report::default();
    match parse(text) {
        Ok(c) => (Some(c), r),
        Err(whole) => {
            let lines = data_lines(text);
            // A first line whose first cell is not a number is the header.
            let header = usize::from(
                lines
                    .first()
                    .and_then(|(_, l)| cells(l).first().map(|c| number(c).is_none()))
                    .unwrap_or(false),
            );
            // The first prefix of the data lines the importer refuses, past the
            // header, names the row.
            let mut named = false;
            for k in (1 + header)..=lines.len() {
                let prefix: Vec<&str> = lines[..k].iter().map(|(_, l)| *l).collect();
                if let Err(why) = parse(&prefix.join("\n")) {
                    let (line_no, line) = lines[k - 1];
                    r.errors.push(Item::row(
                        "table_file",
                        k - 1 - header,
                        format!(
                            "Line {line_no} is not a usable {} row (\"{}\"): {why}.",
                            match engine {
                                EngineKind::Wavsyn => "wavelet",
                                EngineKind::Seds => "damped-sine",
                            },
                            line.trim()
                        ),
                    ));
                    named = true;
                    break;
                }
            }
            if !named {
                r.errors.push(Item::field(
                    "table_file",
                    "path",
                    format!("The file is not a component table: {whole}."),
                ));
            }
            (None, r)
        }
    }
}

/// `srs.target.import`'s result for the file at `path`.
pub fn target_import(path: &str) -> Result<Value, MethodError> {
    let (target, report) = match read(path, "target_file")? {
        Ok(text) => parse_target_text(&text),
        Err(item) => (
            None,
            Report {
                errors: vec![item],
                warnings: Vec::new(),
            },
        ),
    };
    Ok(json!({
        "target": target.as_ref().map_or(Value::Null, WireTarget::to_json),
        "errors": report.errors.iter().map(Item::to_json).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(Item::to_json).collect::<Vec<_>>(),
    }))
}

/// `srs.table.import`'s result for the file at `path`, for `engine`.
pub fn table_import(path: &str, engine: EngineKind) -> Result<Value, MethodError> {
    let (components, report) = match read(path, "table_file")? {
        Ok(text) => parse_table_text(&text, engine),
        Err(item) => (
            None,
            Report {
                errors: vec![item],
                warnings: Vec::new(),
            },
        ),
    };
    Ok(json!({
        "components": components.as_ref().map_or(Value::Null, WireComponents::to_json),
        "errors": report.errors.iter().map(Item::to_json).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(Item::to_json).collect::<Vec<_>>(),
    }))
}

/// `srs.table.import`'s `engine` parameter.
pub fn parse_engine(params: &serde_json::Map<String, Value>) -> Result<EngineKind, MethodError> {
    match crate::protocol::named_str(params, "engine")? {
        Some("wavsyn") => Ok(EngineKind::Wavsyn),
        Some("seds") => Ok(EngineKind::Seds),
        _ => Err(MethodError::bad_params(
            "engine",
            "\"engine\" is \"wavsyn\" or \"seds\": the engine the table is for",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_file_reads_with_a_header_and_comments() {
        let (t, r) = parse_target_text("# a target\nfreq_hz,srs_g\n100,40\n1000,100\n2000,100\n");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(
            t,
            Some(WireTarget::Breakpoints {
                points: vec![(100.0, 40.0), (1000.0, 100.0), (2000.0, 100.0)]
            })
        );
    }

    #[test]
    fn a_bad_target_row_is_named() {
        let (t, r) = parse_target_text("100\t40\n1000\t100\n900\t100\n");
        assert!(t.is_none());
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].reason, "target_file");
        assert_eq!(r.errors[0].at, crate::table::At::Row(2));
        assert!(
            r.errors[0].message.contains("Line 3"),
            "{}",
            r.errors[0].message
        );
    }

    #[test]
    fn a_table_file_reads_and_a_bad_row_is_named() {
        let good = "freq,amp,halfcycles,delay\n100,1,5,0\n200,1,7,0.01\n";
        let (c, r) = parse_table_text(good, EngineKind::Wavsyn);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(c.unwrap().len(), 2);
        let bad = "freq,amp,halfcycles,delay\n100,1,5,0\n200,1,4,0.01\n";
        let (c, r) = parse_table_text(bad, EngineKind::Wavsyn);
        assert!(c.is_none());
        assert_eq!(r.errors[0].reason, "table_file");
        assert_eq!(r.errors[0].at, crate::table::At::Row(1));
        assert!(
            r.errors[0].message.contains("Line 3"),
            "{}",
            r.errors[0].message
        );
    }
}
