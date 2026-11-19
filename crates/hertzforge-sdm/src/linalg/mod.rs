//! Per-line linear algebra on spectral density matrix stacks.
//!
//! Every function here works line by line over a stack, in complex double
//! precision, under the crate's convention `G = E[X X^H]`: a response `Y = H X` has
//! `G_yy = H G_xx H^H` and a response-to-drive cross spectrum `G_yx = E[Y X^H] =
//! H G_xx`.
//!
//! The dense factorizations (Hermitian eigendecomposition, singular value
//! decomposition, Cholesky) come from `faer`, run sequentially: law files are
//! single-threaded WebAssembly, and the lines of a stack are independent, so a
//! caller that wants threads splits the stack. Each call allocates its workspace
//! once and reuses it for every line.
//!
//! Sources, by author, title and venue:
//! - D. O. Smallwood, "Multiple Shaker Random Vibration Control: An Update", Proc.
//!   IEST 1999: the `E[X X^H]` convention, the pseudo-inverse drive, the projection
//!   a plant with more controls than drives achieves.
//! - N. Hunter, K. Cross and G. Nelson, "The Cross Spectrum in Multiple Input
//!   Multiple Response Vibration Testing", IMAC proceedings; and N. J. Higham,
//!   "Computing a Nearest Symmetric Positive Semidefinite Matrix", Linear Algebra
//!   and its Applications 103 (1988): repair of a spectral matrix by eigenvalue
//!   clamping.
//! - R. Schultz, "A Demonstration of Force Estimation and Regularization Methods for
//!   Multi-Shaker Testing", IMAC proceedings: truncated and Tikhonov
//!   pseudo-inverses and the condition-number threshold.
//! - J. S. Bendat and A. G. Piersol, "Random Data: Analysis and Measurement
//!   Procedures", 4th ed., Wiley (2010): the H1 estimator and multiple coherence.

// Index loops mirror the matrix algebra they implement.
#![allow(clippy::needless_range_loop)]

use crate::{HermitianStack, MatrixStack, ShapeError, C64};
use faer::diag::Diag;
use faer::dyn_stack::{MemBuffer, MemStack};
use faer::linalg::cholesky::llt::factor::{
    cholesky_in_place, cholesky_in_place_scratch, LltError, LltRegularization,
};
use faer::linalg::evd::{self_adjoint_evd, self_adjoint_evd_scratch, ComputeEigenvectors};
use faer::linalg::matmul::matmul;
use faer::linalg::svd::{svd as faer_svd, svd_scratch, ComputeSvdVectors};
use faer::{Accum, Mat, MatMut, MatRef, Par};
use serde::{Deserialize, Serialize};
use std::fmt;

#[cfg(test)]
mod tests;

const ONE: C64 = C64::new(1.0, 0.0);
const ZERO: C64 = C64::new(0.0, 0.0);

/// A Cholesky pivot whose square is at or below this fraction of the line's
/// largest diagonal entry is treated as zero: the line is numerically singular and
/// [`cholesky_factor`] refuses it so the caller can fall back to [`sqrt_factor`].
pub const CHOLESKY_PIVOT_REL: f64 = 1e-12;

/// Default clamp threshold for [`psd_repair`]: eigenvalues below `-1e-6 *
/// lambda_max` are clamped (design 8.3, Q30).
pub const PSD_CLAMP_REL: f64 = 1e-6;

/// Default reject threshold for [`psd_repair`]: an eigenvalue below `-1e-3 *
/// lambda_max` fails the call (design 8.3, Q30).
pub const PSD_REJECT_REL: f64 = 1e-3;

/// Why a linear-algebra call failed. Every failure is a value; nothing here panics
/// on a caller's input.
#[derive(Clone, Debug, PartialEq)]
pub enum LinalgError {
    /// A dimension mismatch, or a matrix with a zero dimension.
    Shape(ShapeError),
    /// A parameter outside its domain.
    Parameter(String),
    /// A line holds a NaN or an infinity.
    NonFinite { what: &'static str, line: usize },
    /// The decomposition did not converge on a line.
    NoConvergence { what: &'static str, line: usize },
    /// Cholesky: the line is not (numerically) positive definite; `pivot` is the
    /// first failing pivot.
    NotPositiveDefinite { line: usize, pivot: usize },
    /// PSD repair: a negative eigenvalue beyond the reject threshold.
    NegativeEigenvalue {
        line: usize,
        eigenvalue: f64,
        lambda_max: f64,
        reject_rel: f64,
    },
}

impl fmt::Display for LinalgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinalgError::Shape(e) => write!(f, "{e}"),
            LinalgError::Parameter(s) => f.write_str(s),
            LinalgError::NonFinite { what, line } => {
                write!(f, "{what}: line {line} holds a non-finite value")
            }
            LinalgError::NoConvergence { what, line } => {
                write!(
                    f,
                    "{what}: the decomposition did not converge on line {line}"
                )
            }
            LinalgError::NotPositiveDefinite { line, pivot } => write!(
                f,
                "line {line} is not positive definite (Cholesky pivot {pivot})"
            ),
            LinalgError::NegativeEigenvalue {
                line,
                eigenvalue,
                lambda_max,
                reject_rel,
            } => write!(
                f,
                "line {line} is not positive semi-definite: eigenvalue {eigenvalue:.6e} is \
                 below -{reject_rel:e} times the largest eigenvalue {lambda_max:.6e}"
            ),
        }
    }
}

impl std::error::Error for LinalgError {}

impl From<ShapeError> for LinalgError {
    fn from(e: ShapeError) -> Self {
        LinalgError::Shape(e)
    }
}

fn shape_err(msg: String) -> LinalgError {
    LinalgError::Shape(ShapeError(msg))
}

fn check_dims(what: &str, rows: usize, cols: usize) -> Result<(), LinalgError> {
    if rows == 0 || cols == 0 {
        return Err(shape_err(format!(
            "{what} is {rows} x {cols}; every matrix dimension must be at least 1"
        )));
    }
    Ok(())
}

fn check_lines(what_a: &str, a: usize, what_b: &str, b: usize) -> Result<(), LinalgError> {
    if a != b {
        return Err(shape_err(format!(
            "{what_a} has {a} lines, {what_b} has {b}"
        )));
    }
    Ok(())
}

/// A transformation stack may hold one line (applied to every line) or one per line.
fn check_broadcast(what: &str, t_lines: usize, lines: usize) -> Result<(), LinalgError> {
    if t_lines != 1 && t_lines != lines {
        return Err(shape_err(format!(
            "{what} has {t_lines} lines; it needs 1 (applied to every line) or {lines}"
        )));
    }
    Ok(())
}

#[inline]
fn bline(t_lines: usize, line: usize) -> usize {
    if t_lines == 1 {
        0
    } else {
        line
    }
}

fn all_finite(v: &[C64]) -> bool {
    v.iter().all(|z| z.re.is_finite() && z.im.is_finite())
}

/// One packed Hermitian line into a full column-major matrix.
fn unpack(g: &HermitianStack, line: usize, a: &mut Mat<C64>) {
    let n = g.n();
    let p = g.line(line);
    let mut k = 0;
    for r in 0..n {
        a[(r, r)] = C64::new(p[k].re, 0.0);
        k += 1;
        for c in (r + 1)..n {
            let v = p[k];
            k += 1;
            a[(r, c)] = v;
            a[(c, r)] = v.conj();
        }
    }
}

/// The Hermitian part of a square matrix, packed (upper triangle, row-major).
fn pack(r: MatRef<'_, C64>, out: &mut [C64]) {
    let n = r.nrows();
    let mut k = 0;
    for i in 0..n {
        out[k] = C64::new(r[(i, i)].re, 0.0);
        k += 1;
        for j in (i + 1)..n {
            out[k] = (r[(i, j)] + r[(j, i)].conj()) * 0.5;
            k += 1;
        }
    }
}

/// A matrix into a row-major slice.
fn store(src: MatRef<'_, C64>, out: &mut [C64]) {
    let (m, n) = (src.nrows(), src.ncols());
    for i in 0..m {
        for j in 0..n {
            out[i * n + j] = src[(i, j)];
        }
    }
}

/// Reusable workspace for the Hermitian eigendecomposition of one `n` by `n` line.
struct EighWork {
    n: usize,
    a: Mat<C64>,
    s: Diag<C64>,
    u: Mat<C64>,
    buf: MemBuffer,
}

impl EighWork {
    fn new(n: usize) -> Self {
        let req = self_adjoint_evd_scratch::<C64>(
            n,
            ComputeEigenvectors::Yes,
            Par::Seq,
            Default::default(),
        );
        EighWork {
            n,
            a: Mat::zeros(n, n),
            s: Diag::zeros(n),
            u: Mat::zeros(n, n),
            buf: MemBuffer::new(req),
        }
    }

    /// Decompose one line: eigenvalues ascending in `value`, eigenvectors in the
    /// columns of `u`.
    fn solve(
        &mut self,
        g: &HermitianStack,
        line: usize,
        what: &'static str,
    ) -> Result<(), LinalgError> {
        if !all_finite(g.line(line)) {
            return Err(LinalgError::NonFinite { what, line });
        }
        unpack(g, line, &mut self.a);
        self_adjoint_evd(
            self.a.as_ref(),
            self.s.as_mut(),
            Some(self.u.as_mut()),
            Par::Seq,
            MemStack::new(&mut self.buf),
            Default::default(),
        )
        .map_err(|_| LinalgError::NoConvergence { what, line })
    }

    #[inline]
    fn value(&self, j: usize) -> f64 {
        self.s.column_vector()[j].re
    }

    fn values(&self) -> Vec<f64> {
        (0..self.n).map(|j| self.value(j)).collect()
    }
}

/// Reusable workspace for the thin SVD of one `m` by `n` line.
struct SvdWork {
    m: usize,
    n: usize,
    k: usize,
    vectors: bool,
    s: Diag<C64>,
    u: Mat<C64>,
    v: Mat<C64>,
    buf: MemBuffer,
}

impl SvdWork {
    fn new(m: usize, n: usize, vectors: bool) -> Self {
        let k = m.min(n);
        let which = if vectors {
            ComputeSvdVectors::Thin
        } else {
            ComputeSvdVectors::No
        };
        let req = svd_scratch::<C64>(m, n, which, which, Par::Seq, Default::default());
        let (um, vm) = if vectors {
            (Mat::zeros(m, k), Mat::zeros(n, k))
        } else {
            (Mat::zeros(0, 0), Mat::zeros(0, 0))
        };
        SvdWork {
            m,
            n,
            k,
            vectors,
            s: Diag::zeros(k),
            u: um,
            v: vm,
            buf: MemBuffer::new(req),
        }
    }

    /// Decompose one line: singular values descending, `H = U S V^H` (thin).
    fn solve(
        &mut self,
        h: &MatrixStack,
        line: usize,
        what: &'static str,
    ) -> Result<(), LinalgError> {
        let data = h.line(line);
        if !all_finite(data) {
            return Err(LinalgError::NonFinite { what, line });
        }
        let a = MatRef::from_row_major_slice(data, self.m, self.n);
        let (u, v) = if self.vectors {
            (Some(self.u.as_mut()), Some(self.v.as_mut()))
        } else {
            (None, None)
        };
        faer_svd(
            a,
            self.s.as_mut(),
            u,
            v,
            Par::Seq,
            MemStack::new(&mut self.buf),
            Default::default(),
        )
        .map_err(|_| LinalgError::NoConvergence { what, line })
    }

    #[inline]
    fn sv(&self, j: usize) -> f64 {
        self.s.column_vector()[j].re
    }

    fn singular_values(&self) -> Vec<f64> {
        (0..self.k).map(|j| self.sv(j)).collect()
    }
}

// ---------------------------------------------------------------------------
// Eigendecomposition, repair, square roots
// ---------------------------------------------------------------------------

/// Per-line Hermitian eigendecomposition `G = V diag(lambda) V^H`.
#[derive(Clone, Debug, PartialEq)]
pub struct Eigh {
    /// Real eigenvalues, ascending within each line, line-major (`lines * n`).
    pub values: Vec<f64>,
    /// Unit eigenvectors: column `j` of a line's matrix belongs to that line's
    /// eigenvalue `j`.
    pub vectors: MatrixStack,
}

/// The eigendecomposition of every line of a Hermitian stack.
pub fn eigh(g: &HermitianStack) -> Result<Eigh, LinalgError> {
    let (lines, n) = (g.lines(), g.n());
    check_dims("eigh input", n, n)?;
    let mut w = EighWork::new(n);
    let mut values = vec![0.0; lines * n];
    let mut vectors = MatrixStack::zeros(lines, n, n);
    for line in 0..lines {
        w.solve(g, line, "eigh")?;
        for j in 0..n {
            values[line * n + j] = w.value(j);
        }
        store(w.u.as_ref(), vectors.line_mut(line));
    }
    Ok(Eigh { values, vectors })
}

/// What [`psd_repair`] did.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepairReport {
    /// Per line, the eigenvalues that were below `-clamp_rel * lambda_max` and so
    /// were clamped (0 on a line left as it was).
    pub clamped: Vec<usize>,
    /// The most negative eigenvalue relative to its line's largest, over all lines;
    /// 0 when no line has a negative eigenvalue. Rounding-level negatives that were
    /// not clamped count here too.
    pub worst_relative: f64,
    /// The line holding `worst_relative`.
    pub worst_line: Option<usize>,
}

impl RepairReport {
    /// All clamped eigenvalues over the stack.
    pub fn total_clamped(&self) -> usize {
        self.clamped.iter().sum()
    }

    /// Lines that were rebuilt.
    pub fn lines_repaired(&self) -> usize {
        self.clamped.iter().filter(|&&c| c > 0).count()
    }
}

/// Make every line positive semi-definite, keeping its diagonal (design 8.3).
///
/// Per line: the packed stack is Hermitian by construction (the Hermitian part of a
/// full matrix is taken by [`HermitianStack::from_full`]); eigen-decompose; if any
/// eigenvalue is below `-reject_rel * lambda_max` fail, naming the line and the
/// eigenvalue; if any is below `-clamp_rel * lambda_max`, rebuild the line with
/// every negative eigenvalue set to zero (the nearest positive semi-definite matrix
/// in the Frobenius norm) and count the clamped ones; then restore the line's
/// original diagonal by the positive diagonal congruence
/// [`HermitianStack::diagonal_congruence`], which keeps the matrix positive
/// semi-definite and every pair's coherence as rebuilt. Clamping only raises a
/// diagonal entry, so the restoring factors are at most 1; a negative input
/// diagonal entry is restored to zero.
///
/// Lines with no eigenvalue below the clamp threshold are left bit for bit as they
/// were. On error the stack is unchanged. Defaults: [`PSD_CLAMP_REL`],
/// [`PSD_REJECT_REL`].
pub fn psd_repair(
    g: &mut HermitianStack,
    clamp_rel: f64,
    reject_rel: f64,
) -> Result<RepairReport, LinalgError> {
    if !(clamp_rel.is_finite() && reject_rel.is_finite() && clamp_rel >= 0.0)
        || clamp_rel > reject_rel
    {
        return Err(LinalgError::Parameter(format!(
            "psd_repair needs finite thresholds with 0 <= clamp_rel <= reject_rel, got \
             clamp_rel {clamp_rel:e} and reject_rel {reject_rel:e}"
        )));
    }
    let (lines, n) = (g.lines(), g.n());
    check_dims("psd_repair input", n, n)?;
    let per = HermitianStack::packed_len(n);
    let mut w = EighWork::new(n);
    let mut vs = Mat::<C64>::zeros(n, n);
    let mut rebuilt = Mat::<C64>::zeros(n, n);
    let mut pending: Vec<(usize, Vec<C64>)> = Vec::new();
    let mut scales = vec![1.0; lines * n];
    let mut report = RepairReport {
        clamped: vec![0; lines],
        worst_relative: 0.0,
        worst_line: None,
    };
    for line in 0..lines {
        w.solve(g, line, "psd_repair")?;
        let lmin = w.value(0);
        let lmax = w.value(n - 1);
        if lmin >= 0.0 {
            continue;
        }
        let rel = if lmax > 0.0 {
            lmin / lmax
        } else {
            f64::NEG_INFINITY
        };
        if rel < report.worst_relative {
            report.worst_relative = rel;
            report.worst_line = Some(line);
        }
        if rel < -reject_rel {
            return Err(LinalgError::NegativeEigenvalue {
                line,
                eigenvalue: lmin,
                lambda_max: lmax,
                reject_rel,
            });
        }
        let threshold = -clamp_rel * lmax;
        let count = (0..n).take_while(|&j| w.value(j) < threshold).count();
        if count == 0 {
            continue;
        }
        report.clamped[line] = count;
        for j in 0..n {
            let s = w.value(j).max(0.0).sqrt();
            for i in 0..n {
                vs[(i, j)] = w.u[(i, j)] * s;
            }
        }
        matmul(
            rebuilt.as_mut(),
            Accum::Replace,
            vs.as_ref(),
            vs.adjoint(),
            ONE,
            Par::Seq,
        );
        let mut packed = vec![ZERO; per];
        pack(rebuilt.as_ref(), &mut packed);
        for i in 0..n {
            let want = g.get(line, i, i).re.max(0.0);
            let have = rebuilt[(i, i)].re;
            scales[line * n + i] = if have > 0.0 { want / have } else { 0.0 };
        }
        pending.push((line, packed));
    }
    for (line, packed) in pending {
        let mut k = 0;
        for r in 0..n {
            for c in r..n {
                g.set(line, r, c, packed[k]);
                k += 1;
            }
        }
    }
    g.diagonal_congruence(&scales)?;
    Ok(report)
}

/// Per line, the eigen square root `L = V diag(sqrt(max(lambda, 0)))`, so `L L^H =
/// G` for a positive semi-definite line (design 7.5). It handles rank-deficient
/// lines, where Cholesky fails. Negative eigenvalues are taken as zero, so run
/// [`psd_repair`] first on a request that may not be positive semi-definite.
pub fn sqrt_factor(g: &HermitianStack) -> Result<MatrixStack, LinalgError> {
    let (lines, n) = (g.lines(), g.n());
    check_dims("sqrt_factor input", n, n)?;
    let mut w = EighWork::new(n);
    let mut out = MatrixStack::zeros(lines, n, n);
    for line in 0..lines {
        w.solve(g, line, "sqrt_factor")?;
        let roots: Vec<f64> = (0..n).map(|j| w.value(j).max(0.0).sqrt()).collect();
        let dst = out.line_mut(line);
        for i in 0..n {
            for j in 0..n {
                dst[i * n + j] = w.u[(i, j)] * roots[j];
            }
        }
    }
    Ok(out)
}

/// Per line, the lower-triangular Cholesky factor `L` with `L L^H = G` and a real
/// positive diagonal. A line that is not positive definite, including one whose
/// smallest pivot squared is at or below [`CHOLESKY_PIVOT_REL`] times its largest
/// diagonal entry, is an error naming the line, so the caller can fall back to
/// [`sqrt_factor`].
pub fn cholesky_factor(g: &HermitianStack) -> Result<MatrixStack, LinalgError> {
    let (lines, n) = (g.lines(), g.n());
    check_dims("cholesky_factor input", n, n)?;
    let req = cholesky_in_place_scratch::<C64>(n, Par::Seq, Default::default());
    let mut buf = MemBuffer::new(req);
    let mut a = Mat::<C64>::zeros(n, n);
    let mut out = MatrixStack::zeros(lines, n, n);
    for line in 0..lines {
        if !all_finite(g.line(line)) {
            return Err(LinalgError::NonFinite {
                what: "cholesky_factor",
                line,
            });
        }
        unpack(g, line, &mut a);
        let dmax = (0..n).map(|i| a[(i, i)].re).fold(0.0f64, f64::max);
        cholesky_in_place(
            a.as_mut(),
            LltRegularization::default(),
            Par::Seq,
            MemStack::new(&mut buf),
            Default::default(),
        )
        .map_err(|e| match e {
            LltError::NonPositivePivot { index } => {
                LinalgError::NotPositiveDefinite { line, pivot: index }
            }
        })?;
        for j in 0..n {
            let p = a[(j, j)].re;
            if p.is_nan() || p * p <= CHOLESKY_PIVOT_REL * dmax {
                return Err(LinalgError::NotPositiveDefinite { line, pivot: j });
            }
        }
        // The factor's diagonal is real; the factorization leaves a rounding-level
        // imaginary part there, which is dropped.
        let dst = out.line_mut(line);
        for i in 0..n {
            for j in 0..i {
                dst[i * n + j] = a[(i, j)];
            }
            dst[i * n + i] = C64::new(a[(i, i)].re, 0.0);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// SVD, pseudo-inverse, projection
// ---------------------------------------------------------------------------

/// Per-line thin singular value decomposition `H = U diag(s) V^H`.
#[derive(Clone, Debug, PartialEq)]
pub struct Svd {
    /// Left singular vectors, `lines` by `rows` by `k`, `k = min(rows, cols)`.
    pub u: MatrixStack,
    /// Singular values, descending within each line, line-major (`lines * k`).
    pub s: Vec<f64>,
    /// Right singular vectors, `lines` by `cols` by `k`.
    pub v: MatrixStack,
}

impl Svd {
    /// Singular values per line, `min(rows, cols)`.
    pub fn k(&self) -> usize {
        self.u.cols()
    }
}

/// The thin SVD of every line, any shape.
pub fn svd(h: &MatrixStack) -> Result<Svd, LinalgError> {
    let (lines, m, n) = h.dims();
    check_dims("svd input", m, n)?;
    let mut w = SvdWork::new(m, n, true);
    let k = w.k;
    let mut u = MatrixStack::zeros(lines, m, k);
    let mut v = MatrixStack::zeros(lines, n, k);
    let mut s = vec![0.0; lines * k];
    for line in 0..lines {
        w.solve(h, line, "svd")?;
        for j in 0..k {
            s[line * k + j] = w.sv(j);
        }
        store(w.u.as_ref(), u.line_mut(line));
        store(w.v.as_ref(), v.line_mut(line));
    }
    Ok(Svd { u, s, v })
}

/// How a pseudo-inverse treats small singular values. Every choice first drops the
/// singular values at or below the numerical-rank floor `max(rows, cols) * eps *
/// s_max` (the usual matrix-rank tolerance), which no inverse can use.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Regularization {
    /// The Moore-Penrose pseudo-inverse: invert every singular value above the
    /// floor.
    None,
    /// Tikhonov: `H^+ = (H^H H + mu^2 I)^-1 H^H`, filter factors `s / (s^2 + mu^2)`,
    /// with `mu = lambda_rel * s_max` per line.
    Tikhonov { lambda_rel: f64 },
    /// Truncated SVD: keep singular values above `rcond * s_max` (`rcond` is one
    /// over the target condition number).
    Truncated { rcond: f64 },
    /// Keep the `k` largest singular values (fewer where the line's numerical rank
    /// is lower).
    KeepRank { k: usize },
}

impl Regularization {
    fn check(self) -> Result<(), LinalgError> {
        match self {
            Regularization::None => Ok(()),
            Regularization::Tikhonov { lambda_rel }
                if lambda_rel.is_finite() && lambda_rel >= 0.0 =>
            {
                Ok(())
            }
            Regularization::Truncated { rcond } if rcond.is_finite() && rcond >= 0.0 => Ok(()),
            Regularization::KeepRank { k } if k >= 1 => Ok(()),
            other => Err(LinalgError::Parameter(format!(
                "regularization {other:?} is outside its domain (relative factors finite and \
                 non-negative, k at least 1)"
            ))),
        }
    }
}

/// The inverse filter factors for one line's descending singular values `s`.
/// Returns `(kept, rank)`: the values used and the numerical rank.
fn inverse_factors(
    s: &[f64],
    dim_max: usize,
    reg: Regularization,
    f: &mut [f64],
) -> (usize, usize) {
    let smax = s.first().copied().unwrap_or(0.0);
    let floor = dim_max as f64 * f64::EPSILON * smax;
    let rank = s.iter().take_while(|&&x| x > floor).count();
    f.fill(0.0);
    let kept = match reg {
        Regularization::None => rank,
        Regularization::Truncated { rcond } => {
            let thr = (rcond * smax).max(floor);
            s.iter().take_while(|&&x| x > thr).count()
        }
        Regularization::KeepRank { k } => k.min(rank),
        Regularization::Tikhonov { lambda_rel } => {
            let mu2 = (lambda_rel * smax) * (lambda_rel * smax);
            for j in 0..rank {
                f[j] = s[j] / (s[j] * s[j] + mu2);
            }
            return (rank, rank);
        }
    };
    for j in 0..kept {
        f[j] = 1.0 / s[j];
    }
    (kept, rank)
}

/// Inverse filter factors for a Hermitian line from its eigenvalues (any order):
/// the singular values are `|lambda|`, and each factor carries its eigenvalue's
/// sign, so `G^+ = V diag(f) V^H`.
fn hermitian_inverse_factors(lams: &[f64], reg: Regularization) -> Vec<f64> {
    let n = lams.len();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| lams[b].abs().total_cmp(&lams[a].abs()));
    let s: Vec<f64> = idx.iter().map(|&i| lams[i].abs()).collect();
    let mut fs = vec![0.0; n];
    inverse_factors(&s, n, reg, &mut fs);
    let mut f = vec![0.0; n];
    for (j, &i) in idx.iter().enumerate() {
        f[i] = fs[j] * lams[i].signum();
    }
    f
}

fn cond(smax: f64, smin: f64) -> f64 {
    if smin > 0.0 {
        smax / smin
    } else {
        f64::INFINITY
    }
}

/// What [`pinv`] used on each line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PinvReport {
    /// Singular values per line, `min(rows, cols)`.
    pub k: usize,
    /// All singular values, descending within each line, line-major (`lines * k`).
    /// The first `kept[line]` of a line are the ones the inverse used.
    pub singular_values: Vec<f64>,
    /// Singular values the inverse used, per line (the rank of the regularized
    /// inverse; for Tikhonov, every value above the floor, filtered).
    pub kept: Vec<usize>,
    /// Numerical rank per line (singular values above the floor).
    pub rank: Vec<usize>,
    /// Largest over smallest kept singular value per line (infinite when none).
    pub cond_kept: Vec<f64>,
    /// Largest over smallest singular value per line (infinite when the smallest
    /// is zero).
    pub cond_overall: Vec<f64>,
}

/// The regularized pseudo-inverse of every line, `H^+ = V diag(f) U^H` from the
/// thin SVD: the inverse for a square full-rank line, the least-squares solution
/// for more rows than columns, the minimum-norm solution for fewer. The result is
/// `cols` by `rows` per line.
pub fn pinv(
    h: &MatrixStack,
    reg: Regularization,
) -> Result<(MatrixStack, PinvReport), LinalgError> {
    reg.check()?;
    let (lines, m, n) = h.dims();
    check_dims("pinv input", m, n)?;
    let mut w = SvdWork::new(m, n, true);
    let k = w.k;
    let mut f = vec![0.0; k];
    let mut vf = Mat::<C64>::zeros(n, k);
    let mut out = MatrixStack::zeros(lines, n, m);
    let mut report = PinvReport {
        k,
        singular_values: vec![0.0; lines * k],
        kept: vec![0; lines],
        rank: vec![0; lines],
        cond_kept: vec![0.0; lines],
        cond_overall: vec![0.0; lines],
    };
    for line in 0..lines {
        w.solve(h, line, "pinv")?;
        let s = w.singular_values();
        let (kept, rank) = inverse_factors(&s, m.max(n), reg, &mut f);
        for j in 0..k {
            for i in 0..n {
                vf[(i, j)] = w.v[(i, j)] * f[j];
            }
        }
        matmul(
            MatMut::from_row_major_slice_mut(out.line_mut(line), n, m),
            Accum::Replace,
            vf.as_ref(),
            w.u.adjoint(),
            ONE,
            Par::Seq,
        );
        report.singular_values[line * k..(line + 1) * k].copy_from_slice(&s);
        report.kept[line] = kept;
        report.rank[line] = rank;
        report.cond_kept[line] = if kept > 0 {
            cond(s[0], s[kept - 1])
        } else {
            f64::INFINITY
        };
        report.cond_overall[line] = cond(s[0], s[k - 1]);
    }
    Ok((out, report))
}

/// The response a plant can achieve for a reference: `P G_ref P^H` with `P = H H^+`
/// per line (the regularized pseudo-inverse of [`pinv`]). With more controls than
/// drives `P` is the orthogonal projection onto the column space of `H`
/// (Smallwood); with a square full-rank `H` it is the identity and the result is the
/// reference. `h` is controls by drives, `g_ref` controls by controls.
pub fn projection_achievable(
    h: &MatrixStack,
    g_ref: &HermitianStack,
    reg: Regularization,
) -> Result<HermitianStack, LinalgError> {
    reg.check()?;
    let (lines, c, d) = h.dims();
    check_dims("projection_achievable frf", c, d)?;
    check_lines("frf", lines, "reference", g_ref.lines())?;
    if g_ref.n() != c {
        return Err(shape_err(format!(
            "reference is {} x {}, the frf has {c} controls",
            g_ref.n(),
            g_ref.n()
        )));
    }
    let mut w = SvdWork::new(c, d, true);
    let k = w.k;
    let mut f = vec![0.0; k];
    let mut uphi = Mat::<C64>::zeros(c, k);
    let mut gf = Mat::<C64>::zeros(c, c);
    let mut a = Mat::<C64>::zeros(k, c);
    let mut b = Mat::<C64>::zeros(k, k);
    let mut cm = Mat::<C64>::zeros(c, k);
    let mut r = Mat::<C64>::zeros(c, c);
    let per = HermitianStack::packed_len(c);
    let mut out = vec![ZERO; lines * per];
    for line in 0..lines {
        w.solve(h, line, "projection_achievable")?;
        let s = w.singular_values();
        inverse_factors(&s, c.max(d), reg, &mut f);
        // P = U diag(s f) U^H, so P G P^H = U (Uphi^H G Uphi) U^H with Uphi = U diag(s f).
        for j in 0..k {
            let phi = s[j] * f[j];
            for i in 0..c {
                uphi[(i, j)] = w.u[(i, j)] * phi;
            }
        }
        unpack(g_ref, line, &mut gf);
        matmul(
            a.as_mut(),
            Accum::Replace,
            uphi.adjoint(),
            gf.as_ref(),
            ONE,
            Par::Seq,
        );
        matmul(
            b.as_mut(),
            Accum::Replace,
            a.as_ref(),
            uphi.as_ref(),
            ONE,
            Par::Seq,
        );
        matmul(
            cm.as_mut(),
            Accum::Replace,
            w.u.as_ref(),
            b.as_ref(),
            ONE,
            Par::Seq,
        );
        matmul(
            r.as_mut(),
            Accum::Replace,
            cm.as_ref(),
            w.u.adjoint(),
            ONE,
            Par::Seq,
        );
        pack(r.as_ref(), &mut out[line * per..(line + 1) * per]);
    }
    Ok(HermitianStack::from_packed(lines, c, out)?)
}

// ---------------------------------------------------------------------------
// Products and transformations
// ---------------------------------------------------------------------------

fn congruence_impl(
    t: &MatrixStack,
    g: &HermitianStack,
    t_name: &str,
    g_name: &str,
    broadcast: bool,
) -> Result<HermitianStack, LinalgError> {
    let (t_lines, m, n) = t.dims();
    let lines = g.lines();
    check_dims(t_name, m, n)?;
    if broadcast {
        check_broadcast(t_name, t_lines, lines)?;
    } else {
        check_lines(t_name, t_lines, g_name, lines)?;
    }
    if g.n() != n {
        return Err(shape_err(format!(
            "{t_name} is {m} x {n}, so {g_name} must be {n} x {n}, got {} x {}",
            g.n(),
            g.n()
        )));
    }
    let mut gf = Mat::<C64>::zeros(n, n);
    let mut w = Mat::<C64>::zeros(m, n);
    let mut r = Mat::<C64>::zeros(m, m);
    let per = HermitianStack::packed_len(m);
    let mut out = vec![ZERO; lines * per];
    for line in 0..lines {
        unpack(g, line, &mut gf);
        let tv = MatRef::from_row_major_slice(t.line(bline(t_lines, line)), m, n);
        matmul(w.as_mut(), Accum::Replace, tv, gf.as_ref(), ONE, Par::Seq);
        matmul(
            r.as_mut(),
            Accum::Replace,
            w.as_ref(),
            tv.adjoint(),
            ONE,
            Par::Seq,
        );
        pack(r.as_ref(), &mut out[line * per..(line + 1) * per]);
    }
    Ok(HermitianStack::from_packed(lines, m, out)?)
}

/// `T G T^H` per line: `t` is `m` by `n`, `g` is `n` by `n`, the result `m` by `m`.
/// A congruence keeps a positive semi-definite stack positive semi-definite. `t`
/// may hold one line, applied to every line of `g`, or one per line.
pub fn congruence(t: &MatrixStack, g: &HermitianStack) -> Result<HermitianStack, LinalgError> {
    congruence_impl(t, g, "transformation", "spectral matrix", true)
}

/// The response spectral matrix a drive produces, `G_yy = H G_xx H^H` per line.
/// `h` is controls by drives, `g_xx` drives by drives; their lines must match.
pub fn response_from_drive(
    h: &MatrixStack,
    g_xx: &HermitianStack,
) -> Result<HermitianStack, LinalgError> {
    congruence_impl(h, g_xx, "frf", "drive SDM", false)
}

/// A plant estimate in transformed spaces, `T_r H T_d` per line: `t_resp` is `p`
/// by `controls`, `h` controls by drives, `t_drive` drives by `q` (or absent). Each
/// transformation may hold one line, applied to every line, or one per line.
pub fn frf_transform(
    t_resp: &MatrixStack,
    h: &MatrixStack,
    t_drive: Option<&MatrixStack>,
) -> Result<MatrixStack, LinalgError> {
    let (lines, c, d) = h.dims();
    check_dims("frf", c, d)?;
    let (tr_lines, p, c2) = t_resp.dims();
    check_dims("response transformation", p, c2)?;
    check_broadcast("response transformation", tr_lines, lines)?;
    if c2 != c {
        return Err(shape_err(format!(
            "response transformation is {p} x {c2}, the frf has {c} controls"
        )));
    }
    let q = match t_drive {
        Some(td) => {
            let (td_lines, d2, q) = td.dims();
            check_dims("drive transformation", d2, q)?;
            check_broadcast("drive transformation", td_lines, lines)?;
            if d2 != d {
                return Err(shape_err(format!(
                    "drive transformation is {d2} x {q}, the frf has {d} drives"
                )));
            }
            q
        }
        None => d,
    };
    let mut w = Mat::<C64>::zeros(p, d);
    let mut out = MatrixStack::zeros(lines, p, q);
    for line in 0..lines {
        let trv = MatRef::from_row_major_slice(t_resp.line(bline(tr_lines, line)), p, c);
        let hv = MatRef::from_row_major_slice(h.line(line), c, d);
        matmul(w.as_mut(), Accum::Replace, trv, hv, ONE, Par::Seq);
        match t_drive {
            Some(td) => {
                let tdv = MatRef::from_row_major_slice(td.line(bline(td.lines(), line)), d, q);
                matmul(
                    MatMut::from_row_major_slice_mut(out.line_mut(line), p, q),
                    Accum::Replace,
                    w.as_ref(),
                    tdv,
                    ONE,
                    Par::Seq,
                );
            }
            None => store(w.as_ref(), out.line_mut(line)),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Identification quantities
// ---------------------------------------------------------------------------

fn check_identification(
    g_yx: &MatrixStack,
    g_xx: &HermitianStack,
) -> Result<(usize, usize, usize), LinalgError> {
    let (lines, c, d) = g_yx.dims();
    check_dims("response-drive cross spectrum", c, d)?;
    check_lines(
        "response-drive cross spectrum",
        lines,
        "drive SDM",
        g_xx.lines(),
    )?;
    if g_xx.n() != d {
        return Err(shape_err(format!(
            "drive SDM is {} x {}, the response-drive cross spectrum has {d} drives",
            g_xx.n(),
            g_xx.n()
        )));
    }
    Ok((lines, c, d))
}

/// The H1 estimator in matrix form, `H = G_yx G_xx^+` per line (Bendat and
/// Piersol), with `G_yx = E[Y X^H]` controls by drives and `G_xx` drives by drives.
/// The pseudo-inverse of `G_xx` is taken through its eigendecomposition with the
/// given regularization.
pub fn h1_frf(
    g_yx: &MatrixStack,
    g_xx: &HermitianStack,
    reg: Regularization,
) -> Result<MatrixStack, LinalgError> {
    reg.check()?;
    let (lines, c, d) = check_identification(g_yx, g_xx)?;
    let mut w = EighWork::new(d);
    let mut wm = Mat::<C64>::zeros(c, d);
    let mut out = MatrixStack::zeros(lines, c, d);
    for line in 0..lines {
        w.solve(g_xx, line, "h1_frf")?;
        let f = hermitian_inverse_factors(&w.values(), reg);
        let gyx = MatRef::from_row_major_slice(g_yx.line(line), c, d);
        matmul(
            wm.as_mut(),
            Accum::Replace,
            gyx,
            w.u.as_ref(),
            ONE,
            Par::Seq,
        );
        for j in 0..d {
            for i in 0..c {
                wm[(i, j)] *= f[j];
            }
        }
        matmul(
            MatMut::from_row_major_slice_mut(out.line_mut(line), c, d),
            Accum::Replace,
            wm.as_ref(),
            w.u.adjoint(),
            ONE,
            Par::Seq,
        );
    }
    Ok(out)
}

/// Multiple coherence of each control on all drives, per line:
/// `gamma_i^2 = (G_yx G_xx^+ G_yx^H)_ii / G_yy,ii`, clamped to `[0, 1]`, and 0 where
/// `G_yy,ii` is not positive. Line-major, `lines * controls` values. `g_yy` is
/// controls by controls, `g_yx` controls by drives, `g_xx` drives by drives.
pub fn multiple_coherence(
    g_yy: &HermitianStack,
    g_yx: &MatrixStack,
    g_xx: &HermitianStack,
    reg: Regularization,
) -> Result<Vec<f64>, LinalgError> {
    reg.check()?;
    let (lines, c, d) = check_identification(g_yx, g_xx)?;
    check_lines("response SDM", g_yy.lines(), "drive SDM", lines)?;
    if g_yy.n() != c {
        return Err(shape_err(format!(
            "response SDM is {} x {}, the response-drive cross spectrum has {c} controls",
            g_yy.n(),
            g_yy.n()
        )));
    }
    let mut w = EighWork::new(d);
    let mut wm = Mat::<C64>::zeros(c, d);
    let mut out = vec![0.0; lines * c];
    for line in 0..lines {
        w.solve(g_xx, line, "multiple_coherence")?;
        let f = hermitian_inverse_factors(&w.values(), reg);
        let gyx = MatRef::from_row_major_slice(g_yx.line(line), c, d);
        // (G_yx G_xx^+ G_yx^H)_ii = sum_j f_j |(G_yx V)_ij|^2.
        matmul(
            wm.as_mut(),
            Accum::Replace,
            gyx,
            w.u.as_ref(),
            ONE,
            Par::Seq,
        );
        for i in 0..c {
            let mut num = 0.0;
            for j in 0..d {
                num += f[j] * wm[(i, j)].norm_sqr();
            }
            let den = g_yy.get(line, i, i).re;
            out[line * c + i] = if den > 0.0 {
                (num / den).clamp(0.0, 1.0)
            } else {
                0.0
            };
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Scalars per line
// ---------------------------------------------------------------------------

/// Condition number per line, largest over smallest singular value (infinite when
/// the smallest is zero, including an all-zero line).
pub fn condition_numbers(h: &MatrixStack) -> Result<Vec<f64>, LinalgError> {
    let (lines, m, n) = h.dims();
    check_dims("condition_numbers input", m, n)?;
    let mut w = SvdWork::new(m, n, false);
    let k = w.k;
    let mut out = vec![0.0; lines];
    for line in 0..lines {
        w.solve(h, line, "condition_numbers")?;
        out[line] = cond(w.sv(0), w.sv(k - 1));
    }
    Ok(out)
}

/// The trace of every line.
pub fn trace(g: &HermitianStack) -> Vec<f64> {
    (0..g.lines()).map(|line| g.trace(line)).collect()
}

/// The real diagonals (power spectral densities) of every line, line-major
/// (`lines * n`).
pub fn diagonals(g: &HermitianStack) -> Vec<f64> {
    let mut out = Vec::with_capacity(g.lines() * g.n());
    for line in 0..g.lines() {
        out.extend(g.diagonal(line));
    }
    out
}

/// The dB error `10 log10(measured / reference)` element by element (per line per
/// control, line-major). NaN where the reference is not positive and finite or the
/// measurement is negative or not finite (no defined error there); minus infinity
/// where the measurement is exactly zero against a positive reference.
pub fn db_error(measured: &[f64], reference: &[f64]) -> Result<Vec<f64>, LinalgError> {
    if measured.len() != reference.len() {
        return Err(shape_err(format!(
            "db_error has {} measured values and {} reference values",
            measured.len(),
            reference.len()
        )));
    }
    Ok(measured
        .iter()
        .zip(reference)
        .map(|(&m, &r)| {
            if !(r.is_finite() && r > 0.0 && m.is_finite() && m >= 0.0) {
                f64::NAN
            } else {
                10.0 * (m / r).log10()
            }
        })
        .collect())
}

/// True for the lines inside the half-open band `[lo, hi)`.
pub fn band_mask(lines: usize, band: (usize, usize)) -> Result<Vec<bool>, LinalgError> {
    let (lo, hi) = band;
    if lo >= hi || hi > lines {
        return Err(shape_err(format!(
            "band [{lo}, {hi}) is not inside 0..{lines}"
        )));
    }
    Ok((0..lines).map(|l| l >= lo && l < hi).collect())
}

/// Set every line outside the half-open band `[lo, hi)` to zero.
pub fn zero_out_of_band(g: &mut HermitianStack, band: (usize, usize)) -> Result<(), LinalgError> {
    let mask = band_mask(g.lines(), band)?;
    let factors: Vec<f64> = mask.iter().map(|&m| if m { 1.0 } else { 0.0 }).collect();
    g.scale_lines(&factors)?;
    Ok(())
}
