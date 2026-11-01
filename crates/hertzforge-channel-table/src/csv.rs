//! RFC 4180-ish CSV import/export for the channel table.
//!
//! The column set and order are identical to the TSV form (see
//! [`ChannelTable::FIELD_KEYS`]); CSV differs only in the delimiter and in quoting.
//! A field is quoted when it contains a comma, a double quote, a CR, or an LF; inside
//! a quoted field a literal double quote is doubled (`""`). This is the same subset of
//! RFC 4180 that Excel and Google Sheets emit, so `to_csv` output pastes cleanly and
//! `from_csv` accepts their exports.
//!
//! Like the TSV form, CSV carries channel rows only; the [`ReferenceSpec`] is dropped
//! (it round-trips through JSON, which is the lossless format). `from_csv` returns a
//! table with `reference: None`; re-attach the reference spec after import if needed.
//!
//! [`ChannelTable::FIELD_KEYS`]: crate::ChannelTable::FIELD_KEYS
//! [`ReferenceSpec`]: crate::ReferenceSpec

use crate::{channel_from_ordered_fields, flat_schema_version, ChannelTable, TableError};

/// Header row for the CSV form (same keys/order as the TSV header).
const CSV_HEADER: &str =
    "idx,en,save,stream,iface,iepe_v,units,sens_mv_eu,range,pol,role,group,abort_g,abort_ips,abort_mil,desc,physical,tier";

impl ChannelTable {
    /// Export the channel rows as RFC 4180-ish CSV with a header row. Fields that
    /// contain a comma, quote, CR, or LF are quoted; embedded quotes are doubled.
    pub fn to_csv(&self) -> String {
        let mut out = String::from(CSV_HEADER);
        for c in &self.channels {
            out.push_str("\r\n");
            let fields = crate::channel_to_ordered_fields(c);
            let mut first = true;
            for f in fields.iter() {
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&quote_field(f));
            }
        }
        out
    }

    /// Parse CSV produced by [`ChannelTable::to_csv`] or exported by a spreadsheet.
    /// Tolerant of a present-or-absent header row, CRLF or LF line endings, quoted
    /// fields (including embedded commas, quotes, and newlines), and a trailing blank
    /// line. Rows are the fixed 18-column order; short rows pad trailing empties, so
    /// a schema-v1 export (16 columns) loads and migrates.
    pub fn from_csv(s: &str) -> Result<Self, TableError> {
        let ncols = ChannelTable::FIELD_KEYS.len();
        let records = parse_csv(s)?;
        let mut channels = Vec::new();
        let mut widest = 0usize;
        for rec in records {
            if rec.iter().all(|c| c.trim().is_empty()) {
                continue; // blank line
            }
            // Header detection: first cell resolves to the idx field. Both schema
            // widths count as a full-width header, so a v1 export's header row is
            // skipped rather than parsed as data.
            if crate::is_header_row(&rec.join("\t"), '\t')
                && (rec.len() == ncols || rec.len() == ChannelTable::V1_COLUMNS)
            {
                // Only skip a full-width header; a data row that happens to start with
                // "idx" would be pathological and is not special-cased.
                continue;
            }
            if rec.len() > ncols {
                return Err(TableError::Parse(format!(
                    "expected at most {ncols} columns, got {}",
                    rec.len()
                )));
            }
            widest = widest.max(rec.len());
            let mut owned = rec;
            while owned.len() < ncols {
                owned.push(String::new());
            }
            let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
            channels.push(channel_from_ordered_fields(&refs)?);
        }
        let mut t = ChannelTable {
            schema_version: flat_schema_version(widest),
            channels,
            reference: None,
        };
        t.migrate_to_current()?;
        Ok(t)
    }
}

/// Quote a single CSV field per RFC 4180 if it needs it; otherwise return it as-is.
fn quote_field(f: &str) -> String {
    if f.contains([',', '"', '\r', '\n']) {
        let escaped = f.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        f.to_string()
    }
}

/// Parse a whole CSV document into records of string fields. Handles quoted fields
/// with embedded delimiters/quotes/newlines per RFC 4180. Line endings may be CRLF or
/// LF; a bare CR inside an unquoted field is treated as a line ending too.
pub(crate) fn parse_csv(s: &str) -> Result<Vec<Vec<String>>, TableError> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = s.chars().peekable();
    let mut any_field_on_line = false;

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                other => field.push(other),
            }
        } else {
            match c {
                '"' => {
                    in_quotes = true;
                    any_field_on_line = true;
                }
                ',' => {
                    record.push(std::mem::take(&mut field));
                    any_field_on_line = true;
                }
                '\r' => {
                    // swallow a following \n (CRLF)
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                    any_field_on_line = false;
                }
                '\n' => {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                    any_field_on_line = false;
                }
                other => {
                    field.push(other);
                    any_field_on_line = true;
                }
            }
        }
    }
    if in_quotes {
        return Err(TableError::Parse("unterminated quoted field in CSV".into()));
    }
    // Flush the final field/record if the input did not end with a newline.
    if !field.is_empty() || !record.is_empty() || any_field_on_line {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

/// Parse a single physical CSV line into fields (used by the clipboard paste path for
/// comma-delimited data). Assumes the line contains no embedded newline.
pub(crate) fn parse_csv_line(line: &str) -> Vec<String> {
    match parse_csv(line) {
        Ok(mut recs) if !recs.is_empty() => recs.remove(0),
        _ => line.split(',').map(|s| s.to_string()).collect(),
    }
}
