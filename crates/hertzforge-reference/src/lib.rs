//! `hertzforge-reference`: the reference a multiple-input, multiple-output (MIMO)
//! random test controls to.
//!
//! A [`Reference`] is one object however it was made: an `n` by `n` Hermitian
//! spectral density matrix (SDM) per line on the control grid, the alarm and abort
//! lines per control, the controlled band, the labels of the control space
//! (channel names, or degree-of-freedom names when a response transformation is
//! set), the optional response and drive transformations, and a [`Provenance`]
//! record saying how, from what, when and by which version it was made.
//!
//! Every way of making one ends in [`Reference::validate`]:
//!
//! - every value finite, every dimension consistent, labels unique;
//! - Hermitian (an imported full matrix is refused above a relative error of
//!   `1e-9`; see [`Reference::sdm_from_full`]);
//! - positive semi-definite on every line, with the repair offered through
//!   [`RepairPolicy`] and reported by [`PsdRepairReport`];
//! - a positive diagonal on every line inside the band, and exact zeros outside;
//! - tolerances consistent: each lower line under its upper line, the abort lines
//!   outside the alarm lines, the reference inside its own alarm lines;
//! - transformations of full rank: the response transformation `T_r` (`n` by
//!   `n_c`) full row rank, the drive transformation `T_d` (`n_d` by `n_u`) full
//!   column rank.
//!
//! Each failure is a named [`ReferenceError`] whose message says what to do.
//!
//! # Conventions
//!
//! The spectral matrix is `G = lim (1/T) E[X X^H]` per line (`hertzforge-sdm`'s
//! convention). Much analysis software, and the scientific-Python estimators among
//! it, uses the Bendat and Piersol form, its complex conjugate; the importers
//! conjugate at the edge (see [`npz`]). Line `k` is at `k * df_hz`. The band is the
//! half-open line range `[lo, hi)`.
//!
//! # Entry methods
//!
//! - [`builders::BreakpointReference`]: a PSD breakpoint table per control plus a
//!   coherence and phase per pair.
//! - [`rigid`]: sensor geometry to the six rigid-body degrees of freedom.
//! - [`npz`]: the NumPy specification file (`cpsd`, `f`, optional limits).
//! - [`csv`]: a plain PSD table and a pair table.
//! - [`native`]: the native reference file (a binary payload plus a JSON
//!   sidecar), which the analysis service's converter also writes.

#![forbid(unsafe_code)]

use hertzforge_sdm::linalg::{
    self, eigh, psd_repair, LinalgError, Regularization, PSD_CLAMP_REL, PSD_REJECT_REL,
};
use serde::{Deserialize, Serialize};
use std::fmt;

pub use hertzforge_sdm::law::Tolerances;
pub use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

pub mod builders;
pub mod csv;
pub mod native;
pub mod npz;
pub mod rigid;

#[cfg(test)]
mod tests;

/// The relative Hermitian error above which an imported full matrix is refused.
pub const HERMITIAN_REL_TOL: f64 = 1e-9;

/// This crate's name and version, recorded in every provenance record.
pub fn tool_version() -> String {
    format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// One line whose smallest eigenvalue is below the clamp threshold.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PsdViolation {
    pub line: usize,
    pub freq_hz: f64,
    pub smallest_eigenvalue: f64,
    pub largest_eigenvalue: f64,
}

/// Why a reference was refused. Every message says what to do about it.
#[derive(Clone, Debug, PartialEq)]
pub enum ReferenceError {
    /// A dimension, count or parameter is inconsistent.
    Shape(String),
    /// A label is empty or repeated.
    Labels(String),
    /// A value is NaN or infinite.
    NonFinite { what: String, line: usize },
    /// An imported full matrix is not Hermitian.
    NotHermitian { line: usize, relative_error: f64 },
    /// Lines that are not positive semi-definite.
    NotPositiveSemiDefinite { violations: Vec<PsdViolation> },
    /// The repair was asked for but the line is beyond the policy's reject depth.
    RepairRefused {
        line: usize,
        freq_hz: f64,
        eigenvalue: f64,
        lambda_max: f64,
        reject_rel: f64,
    },
    /// A diagonal entry inside the band is not positive.
    ZeroDiagonalInBand {
        line: usize,
        freq_hz: f64,
        channel: String,
    },
    /// A line outside the band is not zero.
    NonzeroOutOfBand { line: usize, freq_hz: f64 },
    /// The tolerance lines are inconsistent.
    Tolerance(String),
    /// A transformation does not have the rank the control space needs.
    TransformationRank {
        which: &'static str,
        rank: usize,
        needed: usize,
        line: usize,
    },
    /// A transformation is the wrong shape for the reference.
    Transformation(String),
    /// A rigid-body geometry cannot resolve the six degrees of freedom.
    Geometry(String),
    /// A file could not be read or was refused.
    File(String),
    /// A builder input is outside its domain.
    Input(String),
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ReferenceError::*;
        match self {
            Shape(s) | Labels(s) | Tolerance(s) | Transformation(s) | Geometry(s) | File(s)
            | Input(s) => f.write_str(s),
            NonFinite { what, line } => write!(
                f,
                "{what} holds a value that is not a finite number on line {line}; correct the \
                 source so every value is a number"
            ),
            NotHermitian {
                line,
                relative_error,
            } => write!(
                f,
                "the matrix on line {line} is not Hermitian (relative error {relative_error:.3e}, \
                 the limit is {HERMITIAN_REL_TOL:e}); each cross term must be the conjugate of \
                 its mirror, so check that the file holds the full matrix and not one triangle"
            ),
            NotPositiveSemiDefinite { violations } => {
                let first = &violations[0];
                write!(
                    f,
                    "{} line(s) are not positive semi-definite, the first at {:.4} Hz (line {}) \
                     with smallest eigenvalue {:.4e} against largest {:.4e}; the pair coherences \
                     and phases are not mutually consistent there. Correct them, or accept the \
                     repair (eigenvalue clamp then rescale to the specified PSDs) and review the \
                     coherence changes it reports",
                    violations.len(),
                    first.freq_hz,
                    first.line,
                    first.smallest_eigenvalue,
                    first.largest_eigenvalue
                )
            }
            RepairRefused {
                line,
                freq_hz,
                eigenvalue,
                lambda_max,
                reject_rel,
            } => write!(
                f,
                "line {line} ({freq_hz:.4} Hz) has eigenvalue {eigenvalue:.4e} against largest \
                 {lambda_max:.4e}, deeper than the repair policy accepts ({reject_rel:e} of the \
                 largest); correct the pair coherences and phases there, or repair with a policy \
                 that accepts any depth and review the coherence changes"
            ),
            ZeroDiagonalInBand {
                line,
                freq_hz,
                channel,
            } => write!(
                f,
                "control {channel} has no level at {freq_hz:.4} Hz (line {line}), inside the \
                 band; every control needs a positive PSD across the whole band. If the source \
                 is defined on a coarser spacing than the grid, run at the source's spacing or \
                 give the control a level there"
            ),
            NonzeroOutOfBand { line, freq_hz } => write!(
                f,
                "line {line} ({freq_hz:.4} Hz) is outside the band but not zero; widen the band \
                 to include it or set it to zero"
            ),
            TransformationRank {
                which,
                rank,
                needed,
                line,
            } => write!(
                f,
                "the {which} transformation has rank {rank} on line {line}, {needed} is needed; \
                 a dependent row or column makes the virtual space ambiguous, so remove it or \
                 correct the matrix"
            ),
        }
    }
}

impl std::error::Error for ReferenceError {}

impl From<hertzforge_sdm::ShapeError> for ReferenceError {
    fn from(e: hertzforge_sdm::ShapeError) -> Self {
        ReferenceError::Shape(e.0)
    }
}

fn linalg_err(e: LinalgError) -> ReferenceError {
    ReferenceError::Shape(e.to_string())
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

/// How a reference was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// PSD breakpoint tables plus a coherence and phase per pair.
    Breakpoints,
    /// A degree-of-freedom reference with the rigid-body response transformation.
    RigidBody,
    /// The NumPy `.npz` specification file.
    SpecificationNpz,
    /// The MATLAB `.mat` specification file, converted beforehand by the Python
    /// function `hertzforge.io.reference_convert.from_mat` (no service verb does it).
    SpecificationMat,
    /// The plain CSV layout.
    Csv,
    /// A `.hforge` file, converted beforehand by the Python function
    /// `hertzforge.io.reference_convert.from_hforge` (no service verb does it).
    Hforge,
    /// Derived from field or flight records.
    FieldData,
    /// Assembled directly from matrices.
    Direct,
}

/// What a PSD repair changed, kept with the reference.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepairSummary {
    pub lines_repaired: usize,
    pub eigenvalues_clamped: usize,
    /// The most negative eigenvalue relative to its line's largest, before repair.
    pub worst_relative_eigenvalue: f64,
    /// The largest change in any pair's squared coherence, absolute.
    pub max_coherence_change: f64,
    pub clamp_rel: f64,
    pub reject_rel: f64,
}

/// How, from what, when and by which version a reference was made.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub method: Method,
    /// The source: a file name, or a short description of the entered data.
    pub source: String,
    /// SHA-256 of the source file, hex, when there was one.
    #[serde(default)]
    pub source_sha256: Option<String>,
    /// Seconds since the Unix epoch when it was made.
    pub created_unix_secs: u64,
    /// The software and version that made it.
    pub tool: String,
    /// For an import: whether every cross term was conjugated on the way in.
    #[serde(default)]
    pub conjugated_on_import: Option<bool>,
    /// Assumptions the maker relied on, in plain words.
    #[serde(default)]
    pub assumptions: Vec<String>,
    /// The PSD repair, when one was accepted.
    #[serde(default)]
    pub repair: Option<RepairSummary>,
}

impl Provenance {
    /// A record stamped now by this crate.
    pub fn new(method: Method, source: impl Into<String>) -> Self {
        Provenance {
            method,
            source: source.into(),
            source_sha256: None,
            created_unix_secs: now_unix_secs(),
            tool: tool_version(),
            conjugated_on_import: None,
            assumptions: Vec::new(),
            repair: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Transformations
// ---------------------------------------------------------------------------

/// A labeled transformation matrix: one line (applied to every line) or one per
/// line of the grid. `row_labels` and `col_labels` name its two sides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transformation {
    pub matrix: MatrixStack,
    pub row_labels: Vec<String>,
    pub col_labels: Vec<String>,
}

impl Transformation {
    /// A frequency-independent real matrix, `rows` by `cols`, row-major.
    pub fn real(
        rows: usize,
        cols: usize,
        values: &[f64],
        row_labels: Vec<String>,
        col_labels: Vec<String>,
    ) -> Result<Self, ReferenceError> {
        let data = values.iter().map(|&v| C64::new(v, 0.0)).collect();
        let matrix = MatrixStack::from_vec(1, rows, cols, data)?;
        let t = Transformation {
            matrix,
            row_labels,
            col_labels,
        };
        t.check_labels("transformation")?;
        Ok(t)
    }

    fn check_labels(&self, which: &str) -> Result<(), ReferenceError> {
        let (_, r, c) = self.matrix.dims();
        if self.row_labels.len() != r || self.col_labels.len() != c {
            return Err(ReferenceError::Transformation(format!(
                "the {which} transformation is {r} by {c} but has {} row and {} column labels; \
                 give one label per row and per column",
                self.row_labels.len(),
                self.col_labels.len()
            )));
        }
        check_unique(&self.row_labels, &format!("{which} transformation rows"))?;
        check_unique(&self.col_labels, &format!("{which} transformation columns"))?;
        Ok(())
    }

    /// The rank and the condition number of each line.
    pub fn rank_and_condition(&self) -> Result<(Vec<usize>, Vec<f64>), ReferenceError> {
        let (_, report) = linalg::pinv(&self.matrix, Regularization::None).map_err(linalg_err)?;
        Ok((report.rank, report.cond_overall))
    }
}

fn check_unique(labels: &[String], what: &str) -> Result<(), ReferenceError> {
    for (i, l) in labels.iter().enumerate() {
        if l.trim().is_empty() {
            return Err(ReferenceError::Labels(format!(
                "{what}: label {} is empty; name every entry",
                i + 1
            )));
        }
        if labels[..i].contains(l) {
            return Err(ReferenceError::Labels(format!(
                "{what}: the label {l:?} is used twice; give each entry its own name"
            )));
        }
    }
    Ok(())
}

fn max_of(v: &[f64]) -> Option<f64> {
    v.iter()
        .cloned()
        .fold(None, |a: Option<f64>, x| Some(a.map_or(x, |a| a.max(x))))
}

// ---------------------------------------------------------------------------
// Repair
// ---------------------------------------------------------------------------

/// The thresholds a PSD repair runs with, relative to each line's largest
/// eigenvalue: eigenvalues below `-clamp_rel` are clamped to zero and the diagonal
/// restored to the specified PSDs by a positive diagonal congruence; an eigenvalue
/// below `-reject_rel` refuses the repair.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepairPolicy {
    pub clamp_rel: f64,
    pub reject_rel: f64,
}

impl RepairPolicy {
    /// The controller's output-check thresholds: clamp below `1e-6`, refuse below
    /// `1e-3` of the largest eigenvalue. A rounding-level inconsistency is
    /// repaired; a real one is refused.
    pub const STANDARD: RepairPolicy = RepairPolicy {
        clamp_rel: PSD_CLAMP_REL,
        reject_rel: PSD_REJECT_REL,
    };

    /// Clamp below `1e-6` and refuse nothing: for an operator who has seen the
    /// coherence changes the repair reports and accepts them.
    pub const fn any_depth() -> RepairPolicy {
        RepairPolicy {
            clamp_rel: PSD_CLAMP_REL,
            reject_rel: f64::MAX,
        }
    }
}

/// One pair's coherence on one repaired line, before and after.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairCoherenceChange {
    pub row: usize,
    pub col: usize,
    /// Squared coherence `|G_ij|^2 / (G_ii G_jj)` before the repair.
    pub before: f64,
    pub after: f64,
}

/// One line the repair rebuilt.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepairedLine {
    pub line: usize,
    pub freq_hz: f64,
    pub smallest_eigenvalue: f64,
    pub largest_eigenvalue: f64,
    pub clamped: usize,
    pub pairs: Vec<PairCoherenceChange>,
}

/// What [`Reference::repair_psd`] did: the lines, their smallest eigenvalue and
/// the change in each pair's coherence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PsdRepairReport {
    pub lines: Vec<RepairedLine>,
    pub summary: RepairSummary,
}

impl fmt::Display for PsdRepairReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.lines.is_empty() {
            return f.write_str("no line needed repair");
        }
        writeln!(
            f,
            "{} line(s) repaired, {} eigenvalue(s) clamped, largest coherence change {:.4}",
            self.summary.lines_repaired,
            self.summary.eigenvalues_clamped,
            self.summary.max_coherence_change
        )?;
        for l in &self.lines {
            write!(
                f,
                "  {:.4} Hz (line {}): smallest eigenvalue {:.4e}, largest {:.4e};",
                l.freq_hz, l.line, l.smallest_eigenvalue, l.largest_eigenvalue
            )?;
            for p in &l.pairs {
                write!(
                    f,
                    " ({},{}) {:.4} -> {:.4}",
                    p.row, p.col, p.before, p.after
                )?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

/// Squared coherence of a pair on one line, 0 where a diagonal is not positive.
pub fn coherence(g: &HermitianStack, line: usize, i: usize, j: usize) -> f64 {
    let d = g.get(line, i, i).re * g.get(line, j, j).re;
    if d > 0.0 {
        g.get(line, i, j).norm_sqr() / d
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// The reference
// ---------------------------------------------------------------------------

/// What validation measured on a reference that passed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub lines_in_band: usize,
    /// The smallest eigenvalue relative to its line's largest, over the band (a
    /// rounding-level negative is allowed).
    pub min_relative_eigenvalue: f64,
    pub worst_line: Option<usize>,
    /// The largest condition number of the response transformation, if any.
    pub response_condition: Option<f64>,
    /// The largest condition number of the drive transformation, if any.
    pub drive_condition: Option<f64>,
}

/// The reference SDM a MIMO random test controls to, with everything that goes
/// with it. Fields are public for reading; construct through an entry method or
/// [`Reference::from_parts`], each of which validates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    /// The SDM at full level, `n` by `n` per line, `E[X X^H]` convention.
    pub sdm: HermitianStack,
    /// Line spacing, Hz.
    pub df_hz: f64,
    /// The half-open controlled band `[lo, hi)` in lines.
    pub band: (usize, usize),
    /// The control-space labels: channel names, or degree-of-freedom names when a
    /// response transformation is set.
    pub labels: Vec<String>,
    /// The engineering unit of the controls (the SDM is this unit squared per
    /// hertz); free text.
    pub unit: String,
    /// The alarm and abort lines per control, as PSDs (`NaN` for none).
    pub tolerances: Tolerances,
    /// `T_r`, `n` by `n_c`: rows are the reference's labels, columns the physical
    /// control channels. Absent when the reference is written on the channels.
    pub response_transformation: Option<Transformation>,
    /// `T_d`, `n_d` by `n_u`: rows the physical drives, columns the virtual drives.
    pub drive_transformation: Option<Transformation>,
    pub provenance: Provenance,
}

/// The parts of a reference before validation.
#[derive(Clone, Debug)]
pub struct ReferenceParts {
    pub sdm: HermitianStack,
    pub df_hz: f64,
    pub band: (usize, usize),
    pub labels: Vec<String>,
    pub unit: String,
    pub tolerances: Tolerances,
    pub response_transformation: Option<Transformation>,
    pub drive_transformation: Option<Transformation>,
    pub provenance: Provenance,
}

impl Reference {
    /// Assemble and validate. With `repair`, a line that is not positive
    /// semi-definite is repaired under that policy first and the repair is recorded
    /// in the provenance; without it, such a line refuses the reference.
    pub fn from_parts(
        parts: ReferenceParts,
        repair: Option<RepairPolicy>,
    ) -> Result<(Reference, Option<PsdRepairReport>), ReferenceError> {
        let mut r = Reference {
            sdm: parts.sdm,
            df_hz: parts.df_hz,
            band: parts.band,
            labels: parts.labels,
            unit: parts.unit,
            tolerances: parts.tolerances,
            response_transformation: parts.response_transformation,
            drive_transformation: parts.drive_transformation,
            provenance: parts.provenance,
        };
        let report = match repair {
            Some(policy) => {
                r.check_structure()?;
                let rep = r.repair_psd(policy)?;
                if rep.lines.is_empty() {
                    None
                } else {
                    Some(rep)
                }
            }
            None => None,
        };
        r.validate()?;
        Ok((r, report))
    }

    /// The packed SDM of a full square stack, refused when a value is not finite
    /// or a line is not Hermitian within [`HERMITIAN_REL_TOL`].
    pub fn sdm_from_full(full: &MatrixStack) -> Result<HermitianStack, ReferenceError> {
        if let Some(line) = (0..full.lines()).find(|&l| {
            full.line(l)
                .iter()
                .any(|v| !(v.re.is_finite() && v.im.is_finite()))
        }) {
            return Err(ReferenceError::NonFinite {
                what: "the spectral matrix".into(),
                line,
            });
        }
        let mut worst = (0usize, 0.0f64);
        for line in 0..full.lines() {
            let e = full.hermitian_error(line)?;
            if e > worst.1 {
                worst = (line, e);
            }
        }
        if worst.1 > HERMITIAN_REL_TOL {
            return Err(ReferenceError::NotHermitian {
                line: worst.0,
                relative_error: worst.1,
            });
        }
        Ok(HermitianStack::from_full(full)?)
    }

    /// Number of lines on the grid.
    pub fn lines(&self) -> usize {
        self.sdm.lines()
    }

    /// The order of the reference (controls or virtual degrees of freedom).
    pub fn n(&self) -> usize {
        self.sdm.n()
    }

    /// True when the reference is written in a virtual space.
    pub fn is_virtual(&self) -> bool {
        self.response_transformation.is_some()
    }

    /// The frequency of line `k`.
    pub fn freq(&self, k: usize) -> f64 {
        k as f64 * self.df_hz
    }

    /// Dimensions, labels, finiteness, band, tolerance lengths and transformation
    /// shapes and ranks: everything but the spectral checks. Returns the largest
    /// condition number of each transformation present.
    fn check_structure(&self) -> Result<(Option<f64>, Option<f64>), ReferenceError> {
        let (lines, n) = (self.sdm.lines(), self.sdm.n());
        if lines == 0 || n == 0 {
            return Err(ReferenceError::Shape(
                "the reference needs at least one line and one control".into(),
            ));
        }
        if !(self.df_hz.is_finite() && self.df_hz > 0.0) {
            return Err(ReferenceError::Shape(format!(
                "the line spacing must be a positive number of hertz, got {}",
                self.df_hz
            )));
        }
        let (lo, hi) = self.band;
        if lo >= hi || hi > lines {
            return Err(ReferenceError::Shape(format!(
                "the band, lines [{lo}, {hi}), is not a non-empty range inside the {lines} lines \
                 of the grid; set it to the lines the reference is defined on"
            )));
        }
        if self.labels.len() != n {
            return Err(ReferenceError::Labels(format!(
                "the reference is {n} by {n} but has {} labels; give one per control",
                self.labels.len()
            )));
        }
        check_unique(&self.labels, "reference")?;
        if let Some(line) = (0..lines).find(|&l| {
            self.sdm
                .line(l)
                .iter()
                .any(|v| !(v.re.is_finite() && v.im.is_finite()))
        }) {
            return Err(ReferenceError::NonFinite {
                what: "the spectral matrix".into(),
                line,
            });
        }
        let want = lines * n;
        for (name, v) in [
            ("alarm upper", &self.tolerances.alarm_upper),
            ("alarm lower", &self.tolerances.alarm_lower),
            ("abort upper", &self.tolerances.abort_upper),
            ("abort lower", &self.tolerances.abort_lower),
        ] {
            if v.len() != want {
                return Err(ReferenceError::Tolerance(format!(
                    "the {name} line has {} values; {lines} lines by {n} controls need {want}",
                    v.len()
                )));
            }
            if let Some(k) = v.iter().position(|x| x.is_infinite()) {
                return Err(ReferenceError::Tolerance(format!(
                    "the {name} line is infinite on line {} for {}; use NaN for no line",
                    k / n,
                    self.labels[k % n]
                )));
            }
        }
        let mut conds = (None, None);
        if let Some(t) = &self.response_transformation {
            t.check_labels("response")?;
            let (tl, r, c) = t.matrix.dims();
            if tl != 1 && tl != lines {
                return Err(ReferenceError::Transformation(format!(
                    "the response transformation has {tl} lines; give one (frequency \
                     independent) or one per line ({lines})"
                )));
            }
            if r != n || t.row_labels != self.labels {
                return Err(ReferenceError::Transformation(format!(
                    "the response transformation's rows must be the reference's {n} labels in \
                     order; it has {r} rows"
                )));
            }
            if r > c {
                return Err(ReferenceError::Transformation(format!(
                    "the response transformation maps {c} controls to {r} virtual degrees of \
                     freedom; it cannot have more rows than columns, so add control channels or \
                     remove degrees of freedom"
                )));
            }
            if !t.matrix.is_finite() {
                return Err(ReferenceError::NonFinite {
                    what: "the response transformation".into(),
                    line: 0,
                });
            }
            let (ranks, cond) = t.rank_and_condition()?;
            if let Some((line, &rank)) = ranks.iter().enumerate().find(|(_, &k)| k < r) {
                return Err(ReferenceError::TransformationRank {
                    which: "response",
                    rank,
                    needed: r,
                    line,
                });
            }
            conds.0 = max_of(&cond);
        }
        if let Some(t) = &self.drive_transformation {
            t.check_labels("drive")?;
            let (tl, r, c) = t.matrix.dims();
            if tl != 1 && tl != lines {
                return Err(ReferenceError::Transformation(format!(
                    "the drive transformation has {tl} lines; give one (frequency independent) \
                     or one per line ({lines})"
                )));
            }
            if c > r {
                return Err(ReferenceError::Transformation(format!(
                    "the drive transformation maps {c} virtual drives to {r} outputs; it cannot \
                     have more columns than rows"
                )));
            }
            if !t.matrix.is_finite() {
                return Err(ReferenceError::NonFinite {
                    what: "the drive transformation".into(),
                    line: 0,
                });
            }
            let (ranks, cond) = t.rank_and_condition()?;
            if let Some((line, &rank)) = ranks.iter().enumerate().find(|(_, &k)| k < c) {
                return Err(ReferenceError::TransformationRank {
                    which: "drive",
                    rank,
                    needed: c,
                    line,
                });
            }
            conds.1 = max_of(&cond);
        }
        Ok(conds)
    }

    /// The smallest and largest eigenvalue of every line.
    fn eigen_extremes(&self) -> Result<Vec<(f64, f64)>, ReferenceError> {
        let n = self.n();
        let e = eigh(&self.sdm).map_err(linalg_err)?;
        Ok((0..self.lines())
            .map(|l| (e.values[l * n], e.values[l * n + n - 1]))
            .collect())
    }

    /// Run every check. Every entry method ends here.
    pub fn validate(&self) -> Result<ValidationReport, ReferenceError> {
        let conds = self.check_structure()?;
        let (lines, n) = (self.lines(), self.n());
        let (lo, hi) = self.band;
        for line in (0..lo).chain(hi..lines) {
            if self
                .sdm
                .line(line)
                .iter()
                .any(|v| v.re != 0.0 || v.im != 0.0)
            {
                return Err(ReferenceError::NonzeroOutOfBand {
                    line,
                    freq_hz: self.freq(line),
                });
            }
        }
        for line in lo..hi {
            for i in 0..n {
                if self.sdm.get(line, i, i).re <= 0.0 {
                    return Err(ReferenceError::ZeroDiagonalInBand {
                        line,
                        freq_hz: self.freq(line),
                        channel: self.labels[i].clone(),
                    });
                }
            }
        }
        let extremes = self.eigen_extremes()?;
        let mut violations = Vec::new();
        let mut worst = (0.0f64, None);
        for (line, &(lmin, lmax)) in extremes.iter().enumerate().take(hi).skip(lo) {
            let rel = if lmax > 0.0 { lmin / lmax } else { 0.0 };
            if rel < worst.0 {
                worst = (rel, Some(line));
            }
            if rel < -PSD_CLAMP_REL {
                violations.push(PsdViolation {
                    line,
                    freq_hz: self.freq(line),
                    smallest_eigenvalue: lmin,
                    largest_eigenvalue: lmax,
                });
            }
        }
        if !violations.is_empty() {
            return Err(ReferenceError::NotPositiveSemiDefinite { violations });
        }
        self.check_tolerances()?;
        Ok(ValidationReport {
            lines_in_band: hi - lo,
            min_relative_eigenvalue: worst.0,
            worst_line: worst.1,
            response_condition: conds.0,
            drive_condition: conds.1,
        })
    }

    fn check_tolerances(&self) -> Result<(), ReferenceError> {
        let n = self.n();
        let t = &self.tolerances;
        let present = |x: f64| !x.is_nan();
        for line in 0..self.lines() {
            for i in 0..n {
                let k = line * n + i;
                let (au, al, bu, bl) = (
                    t.alarm_upper[k],
                    t.alarm_lower[k],
                    t.abort_upper[k],
                    t.abort_lower[k],
                );
                let at = || {
                    format!(
                        "{} at {:.4} Hz (line {line})",
                        self.labels[i],
                        self.freq(line)
                    )
                };
                for (name, v) in [
                    ("alarm upper", au),
                    ("alarm lower", al),
                    ("abort upper", bu),
                    ("abort lower", bl),
                ] {
                    if present(v) && v < 0.0 {
                        return Err(ReferenceError::Tolerance(format!(
                            "the {name} line is negative for {}; a PSD line is zero or more",
                            at()
                        )));
                    }
                }
                let pairs = [
                    ("alarm lower", al, "alarm upper", au),
                    ("abort lower", bl, "abort upper", bu),
                    ("abort lower", bl, "alarm lower", al),
                    ("alarm upper", au, "abort upper", bu),
                ];
                for (ln, lv, un, uv) in pairs {
                    if present(lv) && present(uv) && lv > uv {
                        return Err(ReferenceError::Tolerance(format!(
                            "for {} the {ln} line ({lv:.4e}) is above the {un} line ({uv:.4e}); \
                             each lower line must be under its upper line and the abort lines \
                             outside the alarm lines",
                            at()
                        )));
                    }
                }
                let g = self.sdm.get(line, i, i).re;
                if g > 0.0 {
                    if present(au) && g > au * (1.0 + 1e-12) {
                        return Err(ReferenceError::Tolerance(format!(
                            "for {} the reference ({g:.4e}) is above its own alarm upper line \
                             ({au:.4e}); widen the line",
                            at()
                        )));
                    }
                    if present(al) && g < al * (1.0 - 1e-12) {
                        return Err(ReferenceError::Tolerance(format!(
                            "for {} the reference ({g:.4e}) is below its own alarm lower line \
                             ({al:.4e}); widen the line",
                            at()
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Make every line positive semi-definite under `policy`, keeping the
    /// specified PSDs, and record the repair in the provenance. Reports each
    /// repaired line, its smallest eigenvalue and every pair's coherence before
    /// and after. On error the reference is unchanged.
    pub fn repair_psd(&mut self, policy: RepairPolicy) -> Result<PsdRepairReport, ReferenceError> {
        let extremes = self.eigen_extremes()?;
        let mut work = self.sdm.clone();
        let rep =
            psd_repair(&mut work, policy.clamp_rel, policy.reject_rel).map_err(|e| match e {
                LinalgError::NegativeEigenvalue {
                    line,
                    eigenvalue,
                    lambda_max,
                    reject_rel,
                } => ReferenceError::RepairRefused {
                    line,
                    freq_hz: self.freq(line),
                    eigenvalue,
                    lambda_max,
                    reject_rel,
                },
                other => linalg_err(other),
            })?;
        let n = self.n();
        let mut lines = Vec::new();
        let mut max_change = 0.0f64;
        for (line, &clamped) in rep.clamped.iter().enumerate() {
            if clamped == 0 {
                continue;
            }
            let mut pairs = Vec::new();
            for i in 0..n {
                for j in i + 1..n {
                    let b = coherence(&self.sdm, line, i, j);
                    let a = coherence(&work, line, i, j);
                    max_change = max_change.max((a - b).abs());
                    pairs.push(PairCoherenceChange {
                        row: i,
                        col: j,
                        before: b,
                        after: a,
                    });
                }
            }
            lines.push(RepairedLine {
                line,
                freq_hz: self.freq(line),
                smallest_eigenvalue: extremes[line].0,
                largest_eigenvalue: extremes[line].1,
                clamped,
                pairs,
            });
        }
        let summary = RepairSummary {
            lines_repaired: rep.lines_repaired(),
            eigenvalues_clamped: rep.total_clamped(),
            worst_relative_eigenvalue: rep.worst_relative,
            max_coherence_change: max_change,
            clamp_rel: policy.clamp_rel,
            reject_rel: policy.reject_rel,
        };
        self.sdm = work;
        if !lines.is_empty() {
            self.provenance.repair = Some(summary.clone());
        }
        Ok(PsdRepairReport { lines, summary })
    }

    /// The reference with a response transformation attached (for example the
    /// rigid-body transformation from [`rigid`]), validated.
    pub fn with_response_transformation(
        mut self,
        t: Transformation,
    ) -> Result<Reference, ReferenceError> {
        self.response_transformation = Some(t);
        self.validate()?;
        Ok(self)
    }

    /// The reference with a drive transformation attached, validated.
    pub fn with_drive_transformation(
        mut self,
        t: Transformation,
    ) -> Result<Reference, ReferenceError> {
        self.drive_transformation = Some(t);
        self.validate()?;
        Ok(self)
    }

    /// The SHA-256 of the reference's content, `sha256:<hex>`: the hash of its
    /// binary encoding in the native file (see [`native`]). Provenance is not part
    /// of it, so the same reference reached two ways hashes the same.
    pub fn content_hash(&self) -> String {
        native::content_hash(&native::encode(self))
    }
}

/// The band of a stack: from the first line with any nonzero diagonal to one past
/// the last. `None` when every line is zero.
pub fn band_of(sdm: &HermitianStack) -> Option<(usize, usize)> {
    let nonzero = |l: usize| sdm.diagonal(l).iter().any(|&d| d != 0.0);
    let lo = (0..sdm.lines()).find(|&l| nonzero(l))?;
    let hi = (0..sdm.lines()).rev().find(|&l| nonzero(l))? + 1;
    Some((lo, hi))
}
