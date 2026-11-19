//! The validation report of a MIMO setup (schema/PROTOCOL.md, "MIMO Random
//! Validation"): the setup's own items, plus what only arithmetic can say, the
//! ranks and conditioning of the transformations, the reference's positive
//! semi-definiteness per line, and feasibility per drive.
//!
//! [`numeric`] is the arithmetic every driver may use; a driver adds its own
//! feasibility verdicts.

use super::driver::LawRef;
use super::setup::{shape_name, ControlSpace, MimoSetup, RealMatrix, Reference};
use crate::protocol::{num, opt_num};
use crate::table::{Item, Report};
use hertzforge_sdm::linalg;
use hertzforge_sdm::{Complex, HermitianStack, MatrixStack};
use serde_json::{json, Value};

/// A transformation's rank and condition number above this warn (the analysis
/// side refuses above the same figure; here it is the operator's call).
pub const CONDITION_WARN: f64 = 100.0;
/// A singular value below this fraction of the largest counts as zero.
pub const RANK_TOL: f64 = 1e-10;
/// The diagonal of a reference must be real to this fraction of its magnitude.
pub const DIAG_IMAG_TOL: f64 = 1e-9;

/// A transformation's readout.
#[derive(Debug, Clone, PartialEq)]
pub struct TransformCheck {
    pub rows: usize,
    pub cols: usize,
    pub rank: usize,
    pub condition: f64,
}

/// The reference's readout.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceCheck {
    pub source: &'static str,
    pub lines: usize,
    pub n: usize,
    pub band_hz: Option<(f64, f64)>,
    /// The most negative eigenvalue relative to its line's largest (0 when none).
    pub min_eigenvalue_ratio: Option<f64>,
    pub rank_min: Option<usize>,
    pub rank_max: Option<usize>,
    /// Lines whose small negative eigenvalues the run will clamp.
    pub repaired_lines: Option<usize>,
}

/// One drive's feasibility verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveFeasibility {
    pub channel: u32,
    pub ceiling_vrms: f64,
    /// `pass`, `advisory`, `exceeded` or `not_evaluated`.
    pub verdict: &'static str,
    pub predicted_vrms: Option<f64>,
    pub message: String,
}

/// The whole report.
#[derive(Debug, Clone, PartialEq)]
pub struct Validation {
    pub report: Report,
    pub controls: usize,
    pub drives: usize,
    pub space: &'static str,
    pub dofs: Option<usize>,
    pub shape: &'static str,
    pub response_transform: Option<TransformCheck>,
    pub drive_transform: Option<TransformCheck>,
    pub reference: Option<ReferenceCheck>,
    pub feasibility: Vec<DriveFeasibility>,
    pub law: Option<LawRef>,
    pub law_admitted: bool,
}

impl Validation {
    pub fn ok(&self) -> bool {
        self.report.errors.is_empty()
    }

    pub fn to_json(&self) -> Value {
        let t = |t: &Option<TransformCheck>| {
            t.as_ref().map_or(Value::Null, |t| {
                json!({"rows": t.rows, "cols": t.cols, "rank": t.rank, "condition": cond(t.condition)})
            })
        };
        json!({
            "ok": self.ok(),
            "errors": self.report.errors.iter().map(Item::to_json).collect::<Vec<_>>(),
            "warnings": self.report.warnings.iter().map(Item::to_json).collect::<Vec<_>>(),
            "shape": {
                "controls": self.controls, "drives": self.drives, "space": self.space,
                "dofs": self.dofs, "shape": self.shape,
            },
            "transforms": {"response": t(&self.response_transform), "drive": t(&self.drive_transform)},
            "reference": self.reference.as_ref().map(|r| json!({
                "source": r.source, "lines": r.lines, "n": r.n,
                "band_hz": r.band_hz.map(|(a, b)| json!([num(a), num(b)])),
                "min_eigenvalue_ratio": r.min_eigenvalue_ratio.map(|x| sig(x, 4)).map_or(Value::Null, num),
                "rank_min": r.rank_min, "rank_max": r.rank_max,
                "repaired_lines": r.repaired_lines,
            })),
            "feasibility": self.feasibility.iter().map(|d| json!({
                "channel": d.channel, "ceiling_vrms": num(d.ceiling_vrms), "verdict": d.verdict,
                "predicted_vrms": opt_num(d.predicted_vrms), "message": d.message,
            })).collect::<Vec<_>>(),
            "law": self.law.as_ref().map(|l| json!({
                "id": l.id, "name": l.name, "version": l.version,
                "sha256": if l.sha256.is_empty() { Value::Null } else { Value::from(l.sha256.as_str()) },
                "admitted": self.law_admitted,
            })),
        })
    }
}

/// A condition number on the wire: rounded to 4 significant digits, null when
/// infinite (a rank-deficient matrix).
fn cond(x: f64) -> Value {
    if x.is_finite() {
        num(sig(x, 4))
    } else {
        Value::Null
    }
}

/// `x` to `digits` significant digits.
pub fn sig(x: f64, digits: i32) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let p = digits - 1 - x.abs().log10().floor() as i32;
    let s = 10f64.powi(p);
    (x * s).round() / s
}

fn transform(m: &RealMatrix) -> TransformCheck {
    let data = m.values.iter().map(|x| Complex::new(*x, 0.0)).collect();
    let (rank, condition) = match MatrixStack::from_vec(1, m.rows, m.cols, data)
        .ok()
        .and_then(|s| linalg::svd(&s).ok())
    {
        Some(svd) => {
            let s = &svd.s;
            let top = s.first().copied().unwrap_or(0.0);
            let rank = s
                .iter()
                .filter(|x| **x > top * RANK_TOL && **x > 0.0)
                .count();
            let low = s.last().copied().unwrap_or(0.0);
            // A rank-deficient matrix has no finite condition number, whatever
            // rounding leaves in its smallest singular value.
            let full = rank == m.rows.min(m.cols);
            (
                rank,
                if full && low > 0.0 {
                    top / low
                } else {
                    f64::INFINITY
                },
            )
        }
        None => (0, f64::INFINITY),
    };
    TransformCheck {
        rows: m.rows,
        cols: m.cols,
        rank,
        condition,
    }
}

/// The arithmetic part of the validation, added to `report` (the setup's own
/// items). Feasibility per drive is left `not_evaluated`: a driver that can predict
/// it replaces those entries.
pub fn numeric(setup: &MimoSetup, mut report: Report, law: Option<LawRef>) -> Validation {
    let (space, dofs, response_transform, drive_transform) = match &setup.space {
        ControlSpace::Physical => ("physical", None, None, None),
        ControlSpace::Virtual {
            dofs,
            response,
            drive,
        } => {
            let r = transform(response);
            let d = drive.as_ref().map(transform);
            for (what, field, t) in [
                ("response", "setup.space.response_transform", Some(&r)),
                ("drive", "setup.space.drive_transform", d.as_ref()),
            ] {
                let Some(t) = t else { continue };
                if t.rank < t.rows.min(t.cols) {
                    report.errors.push(Item::field(
                        "transform_rank_deficient",
                        field,
                        format!(
                            "The {what} transformation has rank {} of {}; its rows are not independent. Remove or correct the dependent rows.",
                            t.rank,
                            t.rows.min(t.cols)
                        ),
                    ));
                } else if t.condition > CONDITION_WARN {
                    report.warnings.push(Item::field(
                        "transform_ill_conditioned",
                        field,
                        format!(
                            "The {what} transformation's condition number is {}; small measurement errors will be amplified. Check the sensor positions and directions.",
                            sig(t.condition, 3)
                        ),
                    ));
                }
            }
            ("virtual", Some(dofs.len()), Some(r), d)
        }
    };
    let reference = Some(reference_check(setup, &mut report));
    let feasibility = setup
        .drives
        .iter()
        .map(|d| DriveFeasibility {
            channel: d.channel,
            ceiling_vrms: d.ceiling_vrms,
            verdict: "not_evaluated",
            predicted_vrms: None,
            message: "The drive this test needs is predicted after system identification, before the loop engages.".into(),
        })
        .collect();
    let law_admitted = law.is_some() && !report.errors.iter().any(|i| i.reason.starts_with("law_"));
    Validation {
        controls: setup.controls.len(),
        drives: setup.drives.len(),
        space,
        dofs,
        shape: shape_name(setup.shape()),
        response_transform,
        drive_transform,
        reference,
        feasibility,
        law,
        law_admitted,
        report,
    }
}

fn reference_check(setup: &MimoSetup, report: &mut Report) -> ReferenceCheck {
    let (freqs, n, re, im) = match &setup.reference {
        Reference::File { .. } => {
            report.warnings.push(Item::field(
                "reference_file_unread",
                "setup.reference.path",
                "The reference file is read and checked when the run starts; validate again then if the file changes.",
            ));
            return ReferenceCheck {
                source: "file",
                lines: setup.lines_hz().len(),
                n: setup.controlled(),
                band_hz: None,
                min_eigenvalue_ratio: None,
                rank_min: None,
                rank_max: None,
                repaired_lines: None,
            };
        }
        Reference::Inline {
            freqs_hz,
            n,
            re,
            im,
        } => (freqs_hz, *n, re, im),
    };
    let mut out = ReferenceCheck {
        source: "inline",
        lines: freqs.len(),
        n,
        band_hz: freqs.first().zip(freqs.last()).map(|(a, b)| (*a, *b)),
        min_eigenvalue_ratio: None,
        rank_min: None,
        rank_max: None,
        repaired_lines: None,
    };
    let per = HermitianStack::packed_len(n);
    if n == 0 || re.len() != freqs.len() * per || im.len() != re.len() {
        return out; // the shape item is already in the report
    }
    // A diagonal is a PSD: real and not negative.
    for (line, f) in freqs.iter().enumerate() {
        for i in 0..n {
            let k = line * per + HermitianStack::packed_index(n, i, i);
            if im[k].abs() > DIAG_IMAG_TOL * re[k].abs().max(f64::MIN_POSITIVE) {
                report.errors.push(Item::field(
                    "reference_not_hermitian",
                    "setup.reference.im",
                    format!(
                        "Line {line} ({} Hz): the diagonal of row {i} has an imaginary part; a PSD is real. Correct the reference.",
                        num(*f)
                    ),
                ));
                return out;
            }
        }
    }
    let data = re
        .iter()
        .zip(im)
        .map(|(r, i)| Complex::new(*r, *i))
        .collect();
    let Ok(stack) = HermitianStack::from_packed(freqs.len(), n, data) else {
        return out;
    };
    let Ok(eig) = linalg::eigh(&stack) else {
        report.errors.push(Item::field(
            "reference_not_psd",
            "setup.reference",
            "The reference's eigenvalues could not be computed; it holds a value that is not finite. Correct the reference.",
        ));
        return out;
    };
    let (mut worst, mut worst_line) = (0.0f64, 0usize);
    let (mut rmin, mut rmax, mut repaired) = (usize::MAX, 0usize, 0usize);
    for line in 0..freqs.len() {
        let v = &eig.values[line * n..(line + 1) * n];
        let top = v.iter().cloned().fold(0.0, f64::max);
        if top <= 0.0 {
            rmin = 0;
            continue;
        }
        let low = v.iter().cloned().fold(f64::INFINITY, f64::min) / top;
        if low < worst {
            worst = low;
            worst_line = line;
        }
        if low < -linalg::PSD_CLAMP_REL {
            repaired += 1;
        }
        let rank = v.iter().filter(|x| **x > top * 1e-9).count();
        rmin = rmin.min(rank);
        rmax = rmax.max(rank);
    }
    out.min_eigenvalue_ratio = Some(worst);
    out.rank_min = Some(if rmin == usize::MAX { 0 } else { rmin });
    out.rank_max = Some(rmax);
    out.repaired_lines = Some(repaired);
    if worst < -linalg::PSD_REJECT_REL {
        report.errors.push(Item::field(
            "reference_not_psd",
            "setup.reference",
            format!(
                "The reference is not positive semi-definite at {} Hz (an eigenvalue is {} of the largest); no signal has these spectra. Lower the coherences there or correct the phases.",
                num(freqs[worst_line]),
                sig(worst, 3)
            ),
        ));
    } else if repaired > 0 {
        report.warnings.push(Item::field(
            "reference_repaired",
            "setup.reference",
            format!(
                "{repaired} line{} of the reference {} slightly negative eigenvalues; the run clamps them to zero and keeps each PSD.",
                if repaired == 1 { "" } else { "s" },
                if repaired == 1 { "has" } else { "have" }
            ),
        ));
    }
    if rmax > setup.drives.len() {
        report.warnings.push(Item::field(
            "reference_rank_above_drives",
            "setup.reference",
            format!(
                "The reference has rank up to {rmax}, more than the {} drive{}; it can be matched only in the least-squares sense.",
                setup.drives.len(),
                if setup.drives.len() == 1 { "" } else { "s" }
            ),
        ));
    }
    out
}
