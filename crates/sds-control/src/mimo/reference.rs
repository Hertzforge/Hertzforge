//! The reference sub-family (schema/PROTOCOL.md, "MIMO Random Reference"): the
//! reference builders, readers and checks of `hertzforge-reference`, on the wire, so
//! the shell shows what the controller computes and never repeats the math.
//!
//! - `mimo_random.reference.build_breakpoints`: one PSD breakpoint table per control
//!   (or degree of freedom) and a coherence and phase per pair, built on the test's
//!   grid, checked line by line for positive semi-definiteness, and repaired only
//!   when the operator accepts the repair the result describes.
//! - `mimo_random.reference.build_rigid_body`: the six rigid-body degrees of
//!   freedom of a sensor geometry: the response transformation, its rank and
//!   condition, and how much of each sensor's reading the others can check.
//! - `mimo_random.reference.validate`: the same checks on a reference the setup
//!   already carries (inline, or a reference folder by path).
//! - `mimo_random.reference.import`: a reference folder, an `.npz` specification or
//!   the CSV layout, read by path on the computer that runs the test.
//!
//! Every answer that carries a reference also carries its preview as views: every
//! PSD on one plot, the smallest eigenvalue per line, and one pair's coherence and
//! phase. A reference small enough travels back inline, ready for the setup; a
//! larger one is written once into the data folder as a reference folder named by
//! its content hash, and travels back by path and hash.

use crate::codec::Arrays;
use crate::mimo::setup::MAX_INLINE_VALUES;
use crate::protocol::{num, MethodError};
use crate::table::{Item, Report};
use crate::views::{self, Axis, Role, Scale, Series};
use hertzforge_breakpoints::{BreakpointRow, BreakpointTable, LevelSpec};
use hertzforge_reference::builders::{
    BreakpointReference, Grid, PairSpec, PairValue, UnspecifiedPairs,
};
use hertzforge_reference::csv::CsvImport;
use hertzforge_reference::npz::NpzImport;
use hertzforge_reference::rigid::{RigidBodyGeometry, Sensor, DOF_LABELS};
use hertzforge_reference::{
    native, PsdRepairReport, PsdViolation, Reference, ReferenceError, RepairPolicy, Transformation,
};
use hertzforge_sdm::{linalg, HermitianStack};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// The most points a preview curve carries; longer ones are reduced chunk by chunk.
pub const PREVIEW_POINTS: usize = 512;
/// The most repaired lines and pair changes a result lists by name.
const LIST_LIMIT: usize = 12;
/// The folder under the data root that large references are written to.
pub const REFERENCE_FOLDER: &str = "references";

// ---- parameters ----------------------------------------------------------------------

fn bad(path: &str, message: impl Into<String>) -> MethodError {
    MethodError::bad_params(path, message)
}

fn opt<'a>(m: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    m.get(key).filter(|v| !v.is_null())
}

fn obj<'a>(v: &'a Value, path: &str) -> Result<&'a Map<String, Value>, MethodError> {
    v.as_object()
        .ok_or_else(|| bad(path, format!("\"{path}\" is an object")))
}

fn list<'a>(v: &'a Value, path: &str) -> Result<&'a Vec<Value>, MethodError> {
    v.as_array()
        .ok_or_else(|| bad(path, format!("\"{path}\" is a list")))
}

fn number(v: &Value, path: &str) -> Result<f64, MethodError> {
    v.as_f64()
        .filter(|x| x.is_finite())
        .ok_or_else(|| bad(path, format!("\"{path}\" is a finite number")))
}

fn opt_number(m: &Map<String, Value>, key: &str, path: &str) -> Result<Option<f64>, MethodError> {
    opt(m, key)
        .map(|v| number(v, &format!("{path}.{key}")))
        .transpose()
}

fn text<'a>(
    m: &'a Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<&'a str>, MethodError> {
    match opt(m, key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(bad(&format!("{path}.{key}"), format!("\"{key}\" is text"))),
    }
}

fn flag(m: &Map<String, Value>, key: &str) -> Result<bool, MethodError> {
    match opt(m, key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(bad(key, format!("\"{key}\" is true or false"))),
    }
}

fn band(m: &Map<String, Value>, key: &str, path: &str) -> Result<Option<(f64, f64)>, MethodError> {
    match opt(m, key) {
        None => Ok(None),
        Some(v) => {
            let p = format!("{path}.{key}");
            let x = list(v, &p)?;
            if x.len() != 2 {
                return Err(bad(&p, format!("\"{key}\" is [below, above] in dB")));
            }
            Ok(Some((number(&x[0], &p)?, number(&x[1], &p)?)))
        }
    }
}

fn triple(v: Option<&Value>, path: &str) -> Result<[f64; 3], MethodError> {
    let v = v.ok_or_else(|| bad(path, format!("\"{path}\" is [x, y, z]")))?;
    let x = list(v, path)?;
    if x.len() != 3 {
        return Err(bad(path, format!("\"{path}\" is [x, y, z]")));
    }
    Ok([
        number(&x[0], path)?,
        number(&x[1], path)?,
        number(&x[2], path)?,
    ])
}

/// The test's grid from its frame: `{"fs_hz","n_pts"}`. Lines run from 0 Hz to the
/// analysis bandwidth `fs / 2.56`, the lines a setup's reference may use.
pub fn grid_of(params: &Map<String, Value>) -> Result<Grid, MethodError> {
    let g = opt(params, "grid").ok_or_else(|| {
        bad(
            "grid",
            "a reference is built on the test's frame: \"grid\" is {\"fs_hz\", \"n_pts\"}",
        )
    })?;
    let g = obj(g, "grid")?;
    let fs = opt_number(g, "fs_hz", "grid")?.unwrap_or(0.0);
    let n_pts = opt(g, "n_pts").and_then(Value::as_u64).unwrap_or(0) as usize;
    if !(256.0..=204_800.0).contains(&fs) {
        return Err(bad("grid.fs_hz", "\"fs_hz\" is from 256 to 204800"));
    }
    if !n_pts.is_power_of_two() || n_pts < 64 {
        return Err(bad("grid.n_pts", "\"n_pts\" is a power of two from 64"));
    }
    let df = fs / n_pts as f64;
    let lines = ((fs / 2.56) / df + 1e-9).floor() as usize + 1;
    Grid::new(df, lines).map_err(|e| bad("grid", e.to_string()))
}

/// One wire breakpoint row (Random's profile row): `{"freq_hz"}` with `psd_g2_hz`
/// or `slope_db_oct`, and optional `alarm_db`, `abort_db`.
fn breakpoint_row(v: &Value, path: &str) -> Result<BreakpointRow, MethodError> {
    let o = obj(v, path)?;
    let freq_hz = opt_number(o, "freq_hz", path)?
        .ok_or_else(|| bad(&format!("{path}.freq_hz"), "each row has its \"freq_hz\""))?;
    let level = match (
        opt_number(o, "psd_g2_hz", path)?,
        opt_number(o, "slope_db_oct", path)?,
    ) {
        (Some(p), None) => LevelSpec::Psd(p),
        (None, Some(s)) => LevelSpec::SlopeDbPerOct(s),
        _ => {
            return Err(bad(
                path,
                "each row has either \"psd_g2_hz\" or \"slope_db_oct\", not both",
            ))
        }
    };
    Ok(BreakpointRow {
        freq_hz,
        level,
        alarm_db: band(o, "alarm_db", path)?,
        abort_db: band(o, "abort_db", path)?,
    })
}

/// A pair quantity: a number, or `[[frequency, value], ...]`.
fn pair_value(v: Option<&Value>, path: &str, default: f64) -> Result<PairValue, MethodError> {
    match v {
        None | Some(Value::Null) => Ok(PairValue::Constant(default)),
        Some(Value::Number(_)) => Ok(PairValue::Constant(number(
            v.unwrap_or(&Value::Null),
            path,
        )?)),
        Some(Value::Array(points)) => {
            let mut out = Vec::with_capacity(points.len());
            for (k, p) in points.iter().enumerate() {
                let pp = format!("{path}[{k}]");
                let xy = list(p, &pp)?;
                if xy.len() != 2 {
                    return Err(bad(&pp, "each point is [frequency, value]"));
                }
                out.push((number(&xy[0], &pp)?, number(&xy[1], &pp)?));
            }
            Ok(PairValue::Table(out))
        }
        Some(_) => Err(bad(
            path,
            "a pair value is a number or a list of [frequency, value] points",
        )),
    }
}

fn labels_of(params: &Map<String, Value>) -> Result<Option<Vec<String>>, MethodError> {
    match opt(params, "labels") {
        None => Ok(None),
        Some(v) => list(v, "labels")?
            .iter()
            .map(|l| {
                l.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| bad("labels", "each label is text"))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
    }
}

// ---- outcomes ------------------------------------------------------------------------

/// What one request made of a reference: the reference if it was accepted, the
/// spectral matrix as entered (for its preview), the line-by-line check and the
/// repair, and every problem as an item.
struct Outcome {
    accepted: Option<Reference>,
    entered: Option<HermitianStack>,
    labels: Vec<String>,
    unit: String,
    df_hz: f64,
    violations: Vec<PsdViolation>,
    repair: Option<PsdRepairReport>,
    repair_accepted: bool,
    report: Report,
    /// A reference folder the reference already lives in (an imported folder).
    folder: Option<PathBuf>,
}

impl Outcome {
    fn empty(labels: Vec<String>, unit: String, df_hz: f64) -> Outcome {
        Outcome {
            accepted: None,
            entered: None,
            labels,
            unit,
            df_hz,
            violations: Vec::new(),
            repair: None,
            repair_accepted: false,
            report: Report::default(),
            folder: None,
        }
    }
}

/// The reason a refusal is filed under.
fn reason_of(e: &ReferenceError) -> &'static str {
    match e {
        ReferenceError::NotPositiveSemiDefinite { .. } | ReferenceError::RepairRefused { .. } => {
            "reference_not_psd"
        }
        ReferenceError::File(_) => "reference_file",
        ReferenceError::Geometry(_) => "geometry",
        ReferenceError::TransformationRank { .. } | ReferenceError::Transformation(_) => {
            "transform_rank"
        }
        ReferenceError::Tolerance(_) => "tolerance",
        _ => "reference_invalid",
    }
}

fn sentence(s: &str) -> String {
    let mut c = s.trim().chars();
    let mut out = match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    };
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

/// Accept `made` (built under no repair), or, when a line is not positive
/// semi-definite, describe the repair and accept it only when `repair` says so.
fn settle(
    out: &mut Outcome,
    made: Result<(Reference, Option<PsdRepairReport>), ReferenceError>,
    repaired: impl FnOnce() -> Result<(Reference, Option<PsdRepairReport>), ReferenceError>,
    repair: bool,
) {
    match made {
        Ok((r, _)) => out.accepted = Some(r),
        Err(ReferenceError::NotPositiveSemiDefinite { violations }) => {
            let lines = violations.len();
            out.violations = violations;
            match repaired() {
                Ok((r, rep)) => {
                    out.repair = rep;
                    if repair {
                        out.repair_accepted = true;
                        out.report.warnings.push(Item::field(
                            "reference_repaired",
                            "reference",
                            format!("{lines} {} repaired: each eigenvalue below zero was set to zero and every PSD kept as entered; the coherence changes are listed.", if lines == 1 { "line was" } else { "lines were" }),
                        ));
                        out.accepted = Some(r);
                    } else {
                        out.report.errors.push(Item::field(
                            "reference_not_psd",
                            "reference",
                            format!("On {lines} {} the coherences and phases cannot all hold at once (no signal has these spectra). Change the coherence or phase of the pairs listed, or accept the repair shown.", if lines == 1 { "line" } else { "lines" }),
                        ));
                    }
                }
                Err(e) => out.report.errors.push(Item::field(
                    reason_of(&e),
                    "reference",
                    sentence(&e.to_string()),
                )),
            }
        }
        Err(e) => out.report.errors.push(Item::field(
            reason_of(&e),
            "reference",
            sentence(&e.to_string()),
        )),
    }
}

// ---- results -------------------------------------------------------------------------

/// The reference for the setup: inline when it is small, otherwise a reference
/// folder in the data folder, by path and hash.
fn wire_reference(out: &Outcome, root: &Path) -> Result<Value, String> {
    let Some(r) = &out.accepted else {
        return Ok(Value::Null);
    };
    let (lo, hi) = r.band;
    let n = r.n();
    let per = HermitianStack::packed_len(n);
    let values = (hi - lo) * per;
    if out.folder.is_none() && values <= MAX_INLINE_VALUES && hi - lo >= 8 {
        let data = &r.sdm.data()[lo * per..hi * per];
        return Ok(json!({
            "kind": "inline",
            "freqs_hz": (lo..hi).map(|k| num(r.freq(k))).collect::<Vec<_>>(),
            "n": n,
            "re": data.iter().map(|c| num(c.re)).collect::<Vec<_>>(),
            "im": data.iter().map(|c| num(c.im)).collect::<Vec<_>>(),
        }));
    }
    let hash = r.content_hash();
    let hex = hash.trim_start_matches("sha256:").to_owned();
    let dir = match &out.folder {
        Some(d) => d.clone(),
        None => {
            let dir = root.join(REFERENCE_FOLDER).join(&hex);
            if !native::paths(&dir).0.exists() {
                native::write(r, &dir).map_err(|e| e.to_string())?;
            }
            dir
        }
    };
    Ok(json!({"kind": "file", "path": dir.display().to_string(), "sha256": hex}))
}

fn transform_json(t: &Option<Transformation>) -> Value {
    match t {
        None => Value::Null,
        Some(t) => {
            let m = &t.matrix;
            let mut values = Vec::with_capacity(m.rows() * m.cols());
            for r in 0..m.rows() {
                for c in 0..m.cols() {
                    values.push(num(m.get(0, r, c).re));
                }
            }
            let (_, cond) = t.rank_and_condition().unwrap_or_default();
            json!({
                "rows": m.rows(), "cols": m.cols(), "values": values,
                "row_labels": t.row_labels, "col_labels": t.col_labels,
                "condition": cond.iter().cloned().fold(None, |a: Option<f64>, c| Some(a.map_or(c, |a| a.max(c)))).map_or(Value::Null, num),
            })
        }
    }
}

/// Reduce `(x, y)` to at most [`PREVIEW_POINTS`] by chunks, keeping each chunk's
/// largest (`keep_max`) or smallest value at the chunk's middle frequency.
fn reduce(x: &[f64], y: &[f64], keep_max: bool) -> (Vec<f64>, Vec<f64>) {
    if x.len() <= PREVIEW_POINTS {
        return (x.to_vec(), y.to_vec());
    }
    let chunk = x.len().div_ceil(PREVIEW_POINTS);
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for (cx, cy) in x.chunks(chunk).zip(y.chunks(chunk)) {
        xs.push(cx[cx.len() / 2]);
        ys.push(cy.iter().cloned().fold(
            if keep_max {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            },
            if keep_max { f64::max } else { f64::min },
        ));
    }
    (xs, ys)
}

fn pair_coherence(g: &HermitianStack, line: usize, i: usize, j: usize) -> (f64, f64) {
    let d = g.diagonal(line);
    let v = g.get(line, i, j);
    let coh = if d[i] > 0.0 && d[j] > 0.0 {
        v.norm_sqr() / (d[i] * d[j])
    } else {
        0.0
    };
    (coh, v.im.atan2(v.re).to_degrees())
}

/// The smallest eigenvalue of each line relative to the line's largest (0 on a
/// line that is all zero).
pub fn min_eigen_ratios(g: &HermitianStack) -> Option<Vec<f64>> {
    let n = g.n();
    let eig = linalg::eigh(g).ok()?;
    Some(
        (0..g.lines())
            .map(|l| {
                let v = &eig.values[l * n..(l + 1) * n];
                let top = v.iter().cloned().fold(0.0, f64::max);
                if top > 0.0 {
                    v.iter().cloned().fold(f64::INFINITY, f64::min) / top
                } else {
                    0.0
                }
            })
            .collect(),
    )
}

fn band_lines(g: &HermitianStack) -> (usize, usize) {
    hertzforge_reference::band_of(g).unwrap_or((0, 0))
}

fn views_of(
    out: &Outcome,
    ratios: Option<&Vec<f64>>,
    pair: (usize, usize),
    arrays: &mut Arrays,
) -> Vec<Value> {
    let shown = out
        .accepted
        .as_ref()
        .map(|r| &r.sdm)
        .or(out.entered.as_ref());
    let Some(g) = shown else {
        return Vec::new();
    };
    let (lo, hi) = band_lines(g);
    if hi <= lo {
        return Vec::new();
    }
    let f: Vec<f64> = (lo..hi).map(|k| k as f64 * out.df_hz).collect();
    let freq = Axis::new("Frequency", Some("Hz"), Scale::Log);
    let mut views = Vec::new();

    // Every PSD on one plot.
    let unit2 = format!("{}²/Hz", out.unit);
    let series = (0..g.n())
        .map(|i| {
            let y: Vec<f64> = (lo..hi).map(|k| g.diagonal(k)[i]).collect();
            let (x, y) = reduce(&f, &y, true);
            Series::new(&out.labels[i], Role::Measured, x, y)
        })
        .collect();
    views.push(views::spectrum(
        arrays,
        "Every PSD",
        &freq,
        &Axis::new("PSD", Some(&unit2), Scale::Log),
        series,
        Some("mimo-reference"),
    ));

    // The smallest eigenvalue per line, of the matrix as entered.
    if let Some(ratios) = ratios {
        let y: Vec<f64> = (lo..hi)
            .map(|k| ratios.get(k).copied().unwrap_or(0.0))
            .collect();
        let (x, y) = reduce(&f, &y, false);
        let floor = vec![-1e-3; x.len()];
        views.push(views::spectrum(
            arrays,
            "Smallest eigenvalue per line",
            &freq,
            &Axis::new("Relative to the line's largest", None, Scale::Linear),
            vec![
                Series::new("As entered", Role::Measured, x.clone(), y),
                Series::new("Refusal threshold", Role::Abort, x, floor),
            ],
            Some("mimo-reference"),
        ));
    }

    // One pair's coherence and phase.
    let (i, j) = pair;
    if i != j && i < g.n() && j < g.n() {
        let name = format!("{} and {}", out.labels[i], out.labels[j]);
        let mut coh = Vec::new();
        let mut phase = Vec::new();
        let entered = out.entered.as_ref();
        let repaired = out.repair_accepted && entered.is_some();
        for (src, label, role) in [
            (
                entered,
                if repaired { "As entered" } else { "Reference" },
                Role::Reference,
            ),
            (
                if repaired {
                    out.accepted.as_ref().map(|r| &r.sdm)
                } else {
                    None
                },
                "After repair",
                Role::Measured,
            ),
        ] {
            let Some(src) = src else { continue };
            let (c, p): (Vec<f64>, Vec<f64>) =
                (lo..hi).map(|k| pair_coherence(src, k, i, j)).unzip();
            let (xc, c) = reduce(&f, &c, false);
            let (xp, p) = reduce(&f, &p, true);
            coh.push(Series::new(label, role, xc, c));
            phase.push(Series::new(label, role, xp, p));
        }
        if !coh.is_empty() {
            views.push(views::spectrum(
                arrays,
                &format!("Coherence, {name}"),
                &freq,
                &Axis::new("Coherence", None, Scale::Linear),
                coh,
                Some("mimo-reference"),
            ));
            views.push(views::spectrum(
                arrays,
                &format!("Phase, {name}"),
                &freq,
                &Axis::new("Phase", Some("deg"), Scale::Linear),
                phase,
                Some("mimo-reference"),
            ));
        }
    }
    views
}

fn repair_json(rep: &PsdRepairReport, labels: &[String]) -> Value {
    // The largest change per pair over the repaired lines.
    let mut worst: Vec<(usize, usize, f64, f64, f64)> = Vec::new();
    for l in &rep.lines {
        for p in &l.pairs {
            let change = (p.after - p.before).abs();
            match worst.iter_mut().find(|w| w.0 == p.row && w.1 == p.col) {
                Some(w) if (w.3 - w.2).abs() >= change => {}
                Some(w) => *w = (p.row, p.col, p.before, p.after, l.freq_hz),
                None => worst.push((p.row, p.col, p.before, p.after, l.freq_hz)),
            }
        }
    }
    worst.sort_by(|a, b| (b.3 - b.2).abs().total_cmp(&(a.3 - a.2).abs()));
    let name = |k: usize| labels.get(k).cloned().unwrap_or_else(|| k.to_string());
    json!({
        "lines": rep.summary.lines_repaired,
        "clamped": rep.summary.eigenvalues_clamped,
        "max_change": num(rep.summary.max_coherence_change),
        "pairs": worst.iter().take(LIST_LIMIT).map(|w| json!({
            "row": w.0, "col": w.1, "labels": [name(w.0), name(w.1)],
            "before": num(w.2), "after": num(w.3), "freq_hz": num(w.4),
        })).collect::<Vec<_>>(),
    })
}

fn result(out: &Outcome, pair: (usize, usize), root: &Path) -> (Value, Arrays) {
    let mut arrays = Arrays::new();
    let mut report = out.report.clone();
    let reference = match wire_reference(out, root) {
        Ok(v) => v,
        Err(e) => {
            report.errors.push(Item::field(
                "reference_file",
                "reference",
                format!("The reference could not be written to the data folder: {e}"),
            ));
            Value::Null
        }
    };
    let entered_ratios = out.entered.as_ref().and_then(min_eigen_ratios);
    let worst = entered_ratios.as_ref().and_then(|r| {
        r.iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(k, v)| (k, *v))
    });
    let band = out
        .accepted
        .as_ref()
        .map(|r| r.band)
        .or_else(|| out.entered.as_ref().map(band_lines));
    let views = views_of(out, entered_ratios.as_ref(), pair, &mut arrays);
    let r = out.accepted.as_ref();
    (
        json!({
            "ok": report.errors.is_empty() && r.is_some(),
            "reference": reference,
            "labels": out.labels,
            "unit": out.unit,
            "df_hz": num(out.df_hz),
            "band_hz": band.filter(|(lo, hi)| hi > lo).map_or(Value::Null, |(lo, hi)| json!([num(lo as f64 * out.df_hz), num((hi - 1) as f64 * out.df_hz)])),
            "lines": band.map_or(0, |(lo, hi)| hi.saturating_sub(lo)),
            "errors": report.errors.iter().map(Item::to_json).collect::<Vec<_>>(),
            "warnings": report.warnings.iter().map(Item::to_json).collect::<Vec<_>>(),
            "psd": {
                "violations": out.violations.len(),
                "min_eigenvalue_ratio": worst.map_or(Value::Null, |w| num(w.1.min(0.0))),
                "worst_hz": worst.filter(|w| w.1 < 0.0).map_or(Value::Null, |w| num(w.0 as f64 * out.df_hz)),
                "lines_hz": out.violations.iter().take(LIST_LIMIT).map(|v| num(v.freq_hz)).collect::<Vec<_>>(),
                "repair": out.repair.as_ref().map_or(Value::Null, |rep| repair_json(rep, &out.labels)),
                "repair_accepted": out.repair_accepted,
            },
            "transforms": {
                "response": r.map_or(Value::Null, |r| transform_json(&r.response_transformation)),
                "drive": r.map_or(Value::Null, |r| transform_json(&r.drive_transformation)),
            },
            "provenance": r.map_or(Value::Null, |r| json!({
                "method": serde_json::to_value(r.provenance.method).unwrap_or(Value::Null),
                "source": r.provenance.source,
                "tool": r.provenance.tool,
                "created_unix_secs": r.provenance.created_unix_secs,
                "conjugated_on_import": r.provenance.conjugated_on_import,
                "assumptions": r.provenance.assumptions,
            })),
            "views": views,
        }),
        arrays,
    )
}

fn preview_pair(params: &Map<String, Value>) -> Result<(usize, usize), MethodError> {
    match opt(params, "preview_pair") {
        None => Ok((0, 1)),
        Some(v) => {
            let x = list(v, "preview_pair")?;
            match (
                x.first().and_then(Value::as_u64),
                x.get(1).and_then(Value::as_u64),
            ) {
                (Some(a), Some(b)) if x.len() == 2 => Ok((a as usize, b as usize)),
                _ => Err(bad(
                    "preview_pair",
                    "\"preview_pair\" is [row, col], from 0",
                )),
            }
        }
    }
}

// ---- the methods ---------------------------------------------------------------------

/// `mimo_random.reference.build_breakpoints`.
pub fn build_breakpoints(
    params: &Map<String, Value>,
    root: &Path,
) -> Result<(Value, Arrays), MethodError> {
    let grid = grid_of(params)?;
    let unit = text(params, "unit", "params")?.unwrap_or("g").to_owned();
    let tables_v = list(
        opt(params, "tables")
            .ok_or_else(|| bad("tables", "one PSD table per control (\"tables\")"))?,
        "tables",
    )?;
    let mut labels = Vec::new();
    let mut psd = Vec::new();
    for (i, t) in tables_v.iter().enumerate() {
        let path = format!("tables[{i}]");
        let o = obj(t, &path)?;
        let label = text(o, "label", &path)?
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| bad(&format!("{path}.label"), "each table names its control"))?
            .to_owned();
        let mut table = BreakpointTable::new(label.clone());
        for (k, r) in list(
            opt(o, "rows").unwrap_or(&Value::Null),
            &format!("{path}.rows"),
        )?
        .iter()
        .enumerate()
        {
            table
                .rows
                .push(breakpoint_row(r, &format!("{path}.rows[{k}]"))?);
        }
        labels.push(label);
        psd.push(table);
    }
    let mut pairs = Vec::new();
    if let Some(v) = opt(params, "pairs") {
        for (k, p) in list(v, "pairs")?.iter().enumerate() {
            let path = format!("pairs[{k}]");
            let o = obj(p, &path)?;
            let idx = |key: &str| {
                opt(o, key)
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                    .ok_or_else(|| {
                        bad(
                            &format!("{path}.{key}"),
                            "a pair names its \"row\" and \"col\", from 0",
                        )
                    })
            };
            pairs.push(PairSpec {
                row: idx("row")?,
                col: idx("col")?,
                coherence: pair_value(opt(o, "coherence"), &format!("{path}.coherence"), 0.0)?,
                phase_deg: pair_value(opt(o, "phase_deg"), &format!("{path}.phase_deg"), 0.0)?,
            });
        }
    }
    let unspecified = match text(params, "unspecified", "params")? {
        None | Some("independent") => UnspecifiedPairs::Independent,
        Some("fully_coherent") => UnspecifiedPairs::FullyCoherent,
        Some(_) => {
            return Err(bad(
                "unspecified",
                "\"unspecified\" is \"independent\" or \"fully_coherent\"",
            ))
        }
    };
    let mut spec = BreakpointReference::new(grid, labels.clone(), psd);
    spec.unit = unit.clone();
    spec.pairs = pairs;
    spec.unspecified = unspecified;
    if let Some(b) = band(params, "alarm_db", "params")? {
        spec.alarm_db = b;
    }
    if let Some(b) = band(params, "abort_db", "params")? {
        spec.abort_db = b;
    }
    let repair = flag(params, "repair")?;
    let mut out = Outcome::empty(labels, unit, grid.df_hz);
    match spec.assemble() {
        Ok(parts) => {
            out.entered = Some(parts.sdm.clone());
            let again = parts.clone();
            settle(
                &mut out,
                Reference::from_parts(parts, None),
                move || Reference::from_parts(again, Some(RepairPolicy::any_depth())),
                repair,
            );
        }
        Err(e) => out.report.errors.push(Item::field(
            reason_of(&e),
            "reference",
            sentence(&e.to_string()),
        )),
    }
    Ok(result(&out, preview_pair(params)?, root))
}

/// `mimo_random.reference.build_rigid_body`.
pub fn build_rigid_body(params: &Map<String, Value>) -> Result<Value, MethodError> {
    let sensors_v = list(
        opt(params, "sensors")
            .ok_or_else(|| bad("sensors", "the control sensors (\"sensors\")"))?,
        "sensors",
    )?;
    let mut sensors = Vec::new();
    for (k, s) in sensors_v.iter().enumerate() {
        let path = format!("sensors[{k}]");
        let o = obj(s, &path)?;
        sensors.push(Sensor {
            label: text(o, "label", &path)?.unwrap_or_default().to_owned(),
            position: triple(opt(o, "position"), &format!("{path}.position"))?,
            direction: triple(opt(o, "direction"), &format!("{path}.direction"))?,
        });
    }
    let geometry = RigidBodyGeometry {
        sensors,
        reference_point: match opt(params, "reference_point") {
            None => [0.0; 3],
            Some(v) => triple(Some(v), "reference_point")?,
        },
        sensor_unit: text(params, "unit", "params")?.unwrap_or("g").to_owned(),
        length_unit: text(params, "length_unit", "params")?
            .unwrap_or("m")
            .to_owned(),
    };
    let fit = match geometry.fit() {
        Ok(f) => f,
        Err(e) => {
            return Ok(json!({
                "ok": false,
                "errors": [Item::field(reason_of(&e), "sensors", sentence(&e.to_string())).to_json()],
                "dofs": DOF_LABELS, "dof_units": Value::Null, "transform": Value::Null,
                "condition": Value::Null, "singular_values": [], "sensors": [],
            }))
        }
    };
    let a = &fit.kinematic;
    let t = &fit.transformation.matrix;
    let n = a.rows();
    // How much of each reading the others can check: the residual projector's
    // diagonal, 1 - (A T_r)_ii. Zero: the fit rests on this sensor alone.
    let sensors_json: Vec<Value> = (0..n)
        .map(|i| {
            let hat: f64 = (0..6).map(|k| (a.get(0, i, k) * t.get(0, k, i)).re).sum();
            json!({"label": fit.transformation.col_labels[i], "check_share": num((1.0 - hat).clamp(0.0, 1.0))})
        })
        .collect();
    let mut warnings = Vec::new();
    if fit.condition > 100.0 {
        warnings.push(Item::field("transform_condition", "sensors", format!("The geometry's condition number is {:.0}: the rotations rest on small differences between sensors. Spread the sensors further apart, or give the positions in a larger length unit.", fit.condition)).to_json());
    }
    Ok(json!({
        "ok": true,
        "errors": [],
        "warnings": warnings,
        "dofs": DOF_LABELS,
        "dof_units": fit.dof_units,
        "transform": transform_json(&Some(fit.transformation.clone())),
        "condition": num(fit.condition),
        "singular_values": fit.singular_values.iter().map(|s| num(*s)).collect::<Vec<_>>(),
        "sensors": sensors_json,
    }))
}

/// `mimo_random.reference.validate`: an inline reference or a reference folder.
pub fn validate(params: &Map<String, Value>, root: &Path) -> Result<(Value, Arrays), MethodError> {
    let r = opt(params, "reference")
        .ok_or_else(|| bad("reference", "the reference to check (\"reference\")"))?;
    let r = obj(r, "reference")?;
    let pair = preview_pair(params)?;
    let unit = text(params, "unit", "params")?.unwrap_or("g").to_owned();
    match text(r, "kind", "reference")? {
        Some("file") => {
            let path = text(r, "path", "reference")?
                .ok_or_else(|| bad("reference.path", "a reference folder is named by its path"))?;
            Ok(result(&read_folder(Path::new(path)), pair, root))
        }
        Some("inline") => {
            let freqs: Vec<f64> = list(
                opt(r, "freqs_hz").unwrap_or(&Value::Null),
                "reference.freqs_hz",
            )?
            .iter()
            .map(|v| number(v, "reference.freqs_hz"))
            .collect::<Result<_, _>>()?;
            let n = opt(r, "n").and_then(Value::as_u64).unwrap_or(0) as usize;
            let re: Vec<f64> = list(opt(r, "re").unwrap_or(&Value::Null), "reference.re")?
                .iter()
                .map(|v| number(v, "reference.re"))
                .collect::<Result<_, _>>()?;
            let im: Vec<f64> = list(opt(r, "im").unwrap_or(&Value::Null), "reference.im")?
                .iter()
                .map(|v| number(v, "reference.im"))
                .collect::<Result<_, _>>()?;
            let per = HermitianStack::packed_len(n);
            if n == 0 || freqs.len() < 2 || re.len() != freqs.len() * per || im.len() != re.len() {
                return Err(bad("reference", "an inline reference carries the upper triangle of each line, real and imaginary parts apart"));
            }
            let df = freqs[1] - freqs[0];
            let lo = (freqs[0] / df).round() as usize;
            let lines = lo + freqs.len();
            let mut data = vec![hertzforge_sdm::C64::new(0.0, 0.0); lo * per];
            data.extend(
                re.iter()
                    .zip(&im)
                    .map(|(a, b)| hertzforge_sdm::C64::new(*a, *b)),
            );
            let labels =
                labels_of(params)?.unwrap_or_else(|| (1..=n).map(|k| k.to_string()).collect());
            let mut out = Outcome::empty(labels.clone(), unit.clone(), df);
            match HermitianStack::from_packed(lines, n, data) {
                Ok(sdm) => {
                    out.entered = Some(sdm.clone());
                    let parts = hertzforge_reference::ReferenceParts {
                        band: band_lines(&sdm),
                        sdm,
                        df_hz: df,
                        labels,
                        unit,
                        tolerances: hertzforge_sdm::law::Tolerances::none(lines, n),
                        response_transformation: None,
                        drive_transformation: None,
                        provenance: hertzforge_reference::Provenance::new(
                            hertzforge_reference::Method::Direct,
                            "the setup's inline reference",
                        ),
                    };
                    let again = parts.clone();
                    settle(
                        &mut out,
                        Reference::from_parts(parts, None),
                        move || Reference::from_parts(again, Some(RepairPolicy::any_depth())),
                        false,
                    );
                }
                Err(e) => out.report.errors.push(Item::field(
                    "reference_invalid",
                    "reference",
                    sentence(&e.to_string()),
                )),
            }
            Ok(result(&out, pair, root))
        }
        _ => Err(bad("reference.kind", "\"kind\" is \"inline\" or \"file\"")),
    }
}

fn read_folder(dir: &Path) -> Outcome {
    let mut out = Outcome::empty(Vec::new(), "g".into(), 1.0);
    match native::read(dir) {
        Ok(r) => {
            out.labels = r.labels.clone();
            out.unit = r.unit.clone();
            out.df_hz = r.df_hz;
            out.entered = Some(r.sdm.clone());
            out.folder = Some(dir.to_path_buf());
            out.accepted = Some(r);
        }
        Err(e) => out.report.errors.push(Item::field(
            reason_of(&e),
            "reference",
            sentence(&e.to_string()),
        )),
    }
    out
}

/// What kind of file a path names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Folder,
    Npz,
    Csv,
    /// A `.mat` or `.hforge` specification: converted to a folder elsewhere first.
    Convert,
    Unknown,
}

pub fn file_kind(path: &Path) -> FileKind {
    if path.is_dir()
        || path
            .file_name()
            .is_some_and(|n| n == "reference.json" || n == "reference.bin")
    {
        return FileKind::Folder;
    }
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("npz") => FileKind::Npz,
        Some("csv") | Some("txt") | Some("tsv") => FileKind::Csv,
        Some("mat") | Some("hforge") => FileKind::Convert,
        _ => FileKind::Unknown,
    }
}

/// `mimo_random.reference.import`.
pub fn import(params: &Map<String, Value>, root: &Path) -> Result<(Value, Arrays), MethodError> {
    let path = text(params, "path", "params")?
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| {
            bad(
                "path",
                "Importing a reference needs the path of its file or folder.",
            )
        })?;
    let path = PathBuf::from(path.trim());
    if !path.is_absolute() {
        return Err(bad(
            "path",
            "Give the full path of the file or folder, on the computer that runs the test.",
        ));
    }
    let pair = preview_pair(params)?;
    let repair = flag(params, "repair")?;
    let unit = text(params, "unit", "params")?.unwrap_or("g").to_owned();
    let labels = labels_of(params)?;
    let df = grid_of(params).map(|g| g.df_hz).unwrap_or(1.0);
    let failed = |message: String| {
        let mut out = Outcome::empty(Vec::new(), unit.clone(), df);
        out.report
            .errors
            .push(Item::field("reference_file", "path", message));
        out
    };
    let out = match file_kind(&path) {
        FileKind::Folder => {
            let dir = if path.is_dir() { path.clone() } else { path.parent().map(Path::to_path_buf).unwrap_or_default() };
            read_folder(&dir)
        }
        FileKind::Npz => {
            let grid = grid_of(params)?;
            let mut opts = NpzImport::new(grid);
            opts.labels = labels;
            opts.unit = unit.clone();
            let mut out = Outcome::empty(Vec::new(), unit.clone(), grid.df_hz);
            let with = |policy: Option<RepairPolicy>| {
                let mut o = opts.clone();
                o.repair = policy;
                hertzforge_reference::npz::read(&path, &o)
            };
            settle(&mut out, with(None), || with(Some(RepairPolicy::any_depth())), repair);
            fill_from(&mut out);
            out
        }
        FileKind::Csv => {
            let grid = grid_of(params)?;
            let psd = std::fs::read_to_string(&path);
            let pairs_path = text(params, "pair_path", "params")?.filter(|p| !p.trim().is_empty());
            let pairs = pairs_path.map(std::fs::read_to_string).transpose();
            match (psd, pairs) {
                (Ok(psd), Ok(pairs)) => {
                    let mut opts = CsvImport::new(grid);
                    opts.unit = unit.clone();
                    let source = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let mut out = Outcome::empty(Vec::new(), unit.clone(), grid.df_hz);
                    let with = |policy: Option<RepairPolicy>| {
                        let mut o = opts.clone();
                        o.repair = policy;
                        hertzforge_reference::csv::import(&psd, pairs.as_deref(), &source, &o)
                    };
                    settle(&mut out, with(None), || with(Some(RepairPolicy::any_depth())), repair);
                    fill_from(&mut out);
                    out
                }
                (Err(e), _) | (_, Err(e)) => failed(format!("The file could not be read: {e}. Check the path.")),
            }
        }
        FileKind::Convert => failed("A .mat or .hforge specification is converted to a reference folder before it is imported here; import the folder, an .npz specification, or a CSV table.".into()),
        FileKind::Unknown => failed("This is not a reference this program reads. Import a reference folder, an .npz specification, or a CSV table.".into()),
    };
    Ok(result(&out, pair, root))
}

/// After an import, the labels, unit and spacing come from the reference itself;
/// the matrix as entered is the one read (before any repair).
fn fill_from(out: &mut Outcome) {
    if let Some(r) = &out.accepted {
        out.labels = r.labels.clone();
        out.unit = r.unit.clone();
        out.df_hz = r.df_hz;
        if out.entered.is_none() && !out.repair_accepted {
            out.entered = Some(r.sdm.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    fn flat(label: &str, psd: f64) -> Value {
        json!({"label": label, "rows": [{"freq_hz": 20, "psd_g2_hz": psd}, {"freq_hz": 500, "psd_g2_hz": psd}]})
    }

    fn root() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sds-mimo-reference-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn independent_pairs_build_inline_on_the_band_lines() {
        let p = params(
            json!({"grid": {"fs_hz": 2048, "n_pts": 256}, "tables": [flat("A", 0.01), flat("B", 0.02)]}),
        );
        let (v, arrays) = build_breakpoints(&p, &root()).unwrap();
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["reference"]["kind"], "inline");
        let n = 2;
        let lines = v["reference"]["freqs_hz"].as_array().unwrap().len();
        assert_eq!(
            v["reference"]["re"].as_array().unwrap().len(),
            lines * n * (n + 1) / 2
        );
        assert_eq!(
            v["reference"]["freqs_hz"][0], 24.0,
            "the band starts on the first line at or above 20 Hz"
        );
        assert_eq!(v["psd"]["violations"], 0);
        assert_eq!(
            v["views"].as_array().unwrap().len(),
            4,
            "PSDs, eigenvalues, coherence, phase"
        );
        assert!(!arrays.is_empty());
    }

    /// Three pairs at coherence 0.9 with phases 0, 0 and 180 degrees cannot hold at
    /// once: refused, with the lines and the repair described; accepted only with
    /// `repair`, every PSD kept.
    #[test]
    fn an_inconsistent_triple_is_refused_until_the_repair_is_accepted() {
        let base = json!({"grid": {"fs_hz": 2048, "n_pts": 256},
            "tables": [flat("A", 0.01), flat("B", 0.01), flat("C", 0.01)],
            "pairs": [{"row": 0, "col": 1, "coherence": 0.9, "phase_deg": 0},
                      {"row": 0, "col": 2, "coherence": 0.9, "phase_deg": 0},
                      {"row": 1, "col": 2, "coherence": 0.9, "phase_deg": 180}]});
        let (v, _) = build_breakpoints(&params(base.clone()), &root()).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["reference"].is_null());
        assert_eq!(v["errors"][0]["reason"], "reference_not_psd");
        assert!(v["psd"]["violations"].as_u64().unwrap() > 0);
        assert!(v["psd"]["min_eigenvalue_ratio"].as_f64().unwrap() < -0.1);
        let rep = &v["psd"]["repair"];
        assert!(rep["lines"].as_u64().unwrap() > 0);
        assert!(rep["pairs"][0]["after"].as_f64().unwrap() < 0.9);
        let mut accept = base;
        accept["repair"] = json!(true);
        let (v, _) = build_breakpoints(&params(accept), &root()).unwrap();
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["psd"]["repair_accepted"], true);
        assert_eq!(v["warnings"][0]["reason"], "reference_repaired");
        // The PSDs are the ones entered: the first line's diagonal is 0.01 each.
        let re = v["reference"]["re"].as_array().unwrap();
        for k in [0usize, 3, 5] {
            assert!(
                (re[k].as_f64().unwrap() - 0.01).abs() < 1e-12,
                "diagonal {k} kept"
            );
        }
    }

    #[test]
    fn a_bad_table_is_an_error_item_not_a_refusal() {
        let p = params(json!({"grid": {"fs_hz": 2048, "n_pts": 256},
            "tables": [{"label": "A", "rows": [{"freq_hz": 100, "slope_db_oct": 3}]}]}));
        let (v, _) = build_breakpoints(&p, &root()).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["errors"].as_array().unwrap().len(), 1);
        assert!(v["errors"][0]["message"].as_str().unwrap().contains("A"));
    }

    #[test]
    fn a_large_reference_travels_as_a_folder_named_by_its_hash() {
        // 24 controls on 981 band lines (1 Hz lines, 20 to 1000 Hz) is more than an
        // inline reference holds (300 values a line, 294,300 in all).
        let tables: Vec<Value> = (0..24)
            .map(|k| json!({"label": format!("C{k}"), "rows": [{"freq_hz": 20, "psd_g2_hz": 0.01}, {"freq_hz": 1000, "psd_g2_hz": 0.01}]}))
            .collect();
        let p = params(json!({"grid": {"fs_hz": 4096, "n_pts": 4096}, "tables": tables}));
        let dir = root();
        let (v, _) = build_breakpoints(&p, &dir).unwrap();
        assert_eq!(v["reference"]["kind"], "file", "{}", v["errors"]);
        let path = PathBuf::from(v["reference"]["path"].as_str().unwrap());
        assert!(path.starts_with(dir.join(REFERENCE_FOLDER)));
        assert_eq!(v["reference"]["sha256"].as_str().unwrap().len(), 64);
        // Imported back, it is the same reference.
        let (w, _) = import(&params(json!({"path": path.display().to_string()})), &dir).unwrap();
        assert_eq!(w["ok"], true, "{w}");
        assert_eq!(w["reference"]["sha256"], v["reference"]["sha256"]);
        assert_eq!(w["labels"].as_array().unwrap().len(), 24);
    }

    fn twelve_sensors() -> Value {
        let mut s = Vec::new();
        for (k, p) in [
            [1.0, 1.0, 0.0],
            [-1.0, 1.0, 0.0],
            [-1.0, -1.0, 0.0],
            [1.0, -1.0, 0.0],
        ]
        .iter()
        .enumerate()
        {
            for (d, axis) in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                .iter()
                .zip(["X", "Y", "Z"])
            {
                s.push(json!({"label": format!("{}{axis}", k + 1), "position": p, "direction": d}));
            }
        }
        Value::Array(s)
    }

    #[test]
    fn twelve_sensors_resolve_six_dofs_and_five_are_refused_by_name() {
        let v = build_rigid_body(&params(
            json!({"sensors": twelve_sensors(), "length_unit": "m"}),
        ))
        .unwrap();
        assert_eq!(v["ok"], true, "{v}");
        assert_eq!(v["transform"]["rows"], 6);
        assert_eq!(v["transform"]["cols"], 12);
        assert_eq!(v["dof_units"][3], "g/m");
        let shares: Vec<f64> = v["sensors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["check_share"].as_f64().unwrap())
            .collect();
        // Twelve readings, six unknowns: the shares sum to the six spare readings.
        assert!(
            (shares.iter().sum::<f64>() - 6.0).abs() < 1e-9,
            "{shares:?}"
        );
        let five: Vec<Value> = twelve_sensors().as_array().unwrap()[..5].to_vec();
        let v = build_rigid_body(&params(json!({"sensors": five}))).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["errors"][0]["reason"], "geometry");
        assert!(v["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("at least six"));
    }

    #[test]
    fn imports_say_what_is_accepted() {
        let dir = root();
        let p = params(json!({"path": dir.join("spec.mat").display().to_string()}));
        let (v, _) = import(&p, &dir).unwrap();
        assert_eq!(v["ok"], false);
        assert!(v["errors"][0]["message"].as_str().unwrap().contains(".npz"));
        assert!(import(&params(json!({"path": "relative/ref"})), &dir).is_err());
    }

    #[test]
    fn the_inline_check_finds_the_negative_line() {
        // Two lines of a 2 by 2 reference; the second has |G01|² > G00 G11.
        let p = params(
            json!({"reference": {"kind": "inline", "freqs_hz": [10, 20], "n": 2,
            "re": [1, 0.5, 1, 1, 2, 1], "im": [0, 0, 0, 0, 0, 0]}}),
        );
        let (v, _) = validate(&p, &root()).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["psd"]["violations"], 1);
        assert_eq!(v["psd"]["worst_hz"], 20.0);
    }

    #[test]
    fn reduce_keeps_the_extreme_of_each_chunk() {
        let x: Vec<f64> = (0..2000).map(f64::from).collect();
        let mut y = vec![0.0; 2000];
        y[1234] = -5.0;
        let (xs, ys) = reduce(&x, &y, false);
        assert!(xs.len() <= PREVIEW_POINTS);
        assert!(
            ys.contains(&-5.0),
            "a single negative line survives the reduction"
        );
    }
}
