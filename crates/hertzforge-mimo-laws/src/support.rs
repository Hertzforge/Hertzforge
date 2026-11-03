//! What the built-in laws share: the parameter and diagnostic declarations
//! they have in common, the regularized inverse of the plant (cached while the
//! plant estimate is unchanged), small per-line matrix products, and the
//! per-loop change measure.
//!
//! Sources, by author, title and venue:
//! - D. O. Smallwood, "Multiple Shaker Random Vibration Control: An Update",
//!   Proc. IEST 1999: the pseudo-inverse drive `G_uu = H^+ G_ref H^+^H`, least
//!   squares with more controls than drives and minimum norm with fewer.
//! - R. Schultz, "A Demonstration of Force Estimation and Regularization
//!   Methods for Multi-Shaker Testing", IMAC proceedings: truncated and
//!   Tikhonov regularization, and the condition-number limit of about 1,000.

use crate::spec::{DiagDecl, DiagKind, ParamDecl, ParamKindDecl};
use hertzforge_sdm::law::LawError;
use hertzforge_sdm::linalg::{self, Regularization};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

pub(crate) const ZERO: C64 = C64::new(0.0, 0.0);

/// What a singular in-band line reports as its condition number: a finite
/// stand-in for infinity, so the diagnostic stays finite.
pub(crate) const SINGULAR_CONDITION: f64 = 1e300;

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

pub(crate) const REGULARIZATIONS: &[&str] = &["truncated", "tikhonov", "none"];
pub(crate) const WEIGHTINGS: &[&str] = &["none", "reference"];

pub(crate) fn regularization_params() -> Vec<ParamDecl> {
    vec![
        ParamDecl {
            key: "regularization",
            label: "Regularization",
            help: "How the inverse of the plant treats weak directions. Truncated drops \
                   singular values below the amount times the largest; Tikhonov damps every \
                   one with the amount times the largest as the damping; none inverts every \
                   singular value above the numerical floor.",
            kind: ParamKindDecl::Choice {
                options: REGULARIZATIONS,
                default: "truncated",
            },
        },
        ParamDecl {
            key: "regularization_amount",
            label: "Regularization amount",
            help: "Relative to the largest singular value on each line. For truncation, 1e-3 \
                   keeps the inverse's condition number under 1,000. Ignored with no \
                   regularization.",
            kind: ParamKindDecl::Number {
                unit: "",
                min: 0.0,
                max: 1.0,
                step: 1e-4,
                default: 1e-3,
            },
        },
    ]
}

pub(crate) fn weighting_param() -> ParamDecl {
    ParamDecl {
        key: "weighting",
        label: "Error weighting",
        help: "How the least-squares inverse weighs each control's error when there are more \
               controls than drives: equally, or relative to its reference PSD so a quiet \
               control counts as much as a loud one. No effect on a square plant.",
        kind: ParamKindDecl::Choice {
            options: WEIGHTINGS,
            default: "none",
        },
    }
}

pub(crate) fn convergence_param() -> ParamDecl {
    ParamDecl {
        key: "convergence",
        label: "Convergence factor",
        help: "The fraction of each loop's correction applied. 1 applies it whole; smaller \
               values converge more slowly and average estimation noise.",
        kind: ParamKindDecl::Number {
            unit: "",
            min: 0.1,
            max: 1.0,
            step: 0.05,
            default: 0.5,
        },
    }
}

pub(crate) fn cap_params() -> Vec<ParamDecl> {
    vec![
        ParamDecl {
            key: "cap_up_db",
            label: "Largest rise per loop",
            help: "The most any drive's PSD, or any factor diagonal's square, may rise in one \
                   loop. Never looser than the controller's own cap.",
            kind: ParamKindDecl::Number {
                unit: "dB",
                min: 0.1,
                max: 3.5,
                step: 0.1,
                default: 3.5,
            },
        },
        ParamDecl {
            key: "cap_down_db",
            label: "Largest fall per loop",
            help: "The most any drive's PSD, or any factor diagonal's square, may fall in one \
                   loop. Never looser than the controller's own cap.",
            kind: ParamKindDecl::Number {
                unit: "dB",
                min: 0.1,
                max: 6.0,
                step: 0.1,
                default: 6.0,
            },
        },
    ]
}

pub(crate) fn rank_tolerance_param() -> ParamDecl {
    ParamDecl {
        key: "rank_tolerance",
        label: "Rank tolerance",
        help: "Singular values of the plant above this fraction of the largest count toward \
               its rank. The reference factor keeps no more independent sources than the \
               rank.",
        kind: ParamKindDecl::Number {
            unit: "",
            min: 0.0,
            max: 0.1,
            step: 1e-6,
            default: 1e-6,
        },
    }
}

pub(crate) fn diag_condition() -> DiagDecl {
    DiagDecl {
        key: "condition",
        label: "Condition number",
        unit: "",
        help: "Largest over smallest singular value of the plant on each line (weighted when \
               the law weights). 0 out of band; 1e300 on a singular line.",
        kind: DiagKind::PerLine,
    }
}

pub(crate) fn diag_kept() -> DiagDecl {
    DiagDecl {
        key: "kept",
        label: "Singular values kept",
        unit: "",
        help: "How many singular values of the plant the inverse used on each line.",
        kind: DiagKind::PerLine,
    }
}

pub(crate) fn diag_correction() -> DiagDecl {
    DiagDecl {
        key: "correction_db",
        label: "Largest drive change",
        unit: "dB",
        help: "The largest change of any drive's PSD on any line from the law's previous \
               drive. 0 at the seed.",
        kind: DiagKind::Scalar,
    }
}

pub(crate) fn diag_clamped() -> DiagDecl {
    DiagDecl {
        key: "clamped",
        label: "Clamped factor diagonals",
        unit: "",
        help: "Diagonal elements of the reference factor whose correction this loop was held \
               to the per-loop limit, over all lines.",
        kind: DiagKind::Scalar,
    }
}

/// The regularization a law's two parameters name.
pub(crate) fn regularization(method: &str, amount: f64) -> Result<Regularization, LawError> {
    match method {
        "truncated" => Ok(Regularization::Truncated { rcond: amount }),
        "tikhonov" => Ok(Regularization::Tikhonov { lambda_rel: amount }),
        "none" => Ok(Regularization::None),
        other => Err(LawError(format!(
            "regularization `{other}` is not one of {}",
            REGULARIZATIONS.join(", ")
        ))),
    }
}

/// How the least-squares inverse weighs each control's error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Weighting {
    /// Every control alike.
    None,
    /// Each control by one over its reference PSD.
    Reference,
}

pub(crate) fn weighting(choice: &str) -> Result<Weighting, LawError> {
    match choice {
        "none" => Ok(Weighting::None),
        "reference" => Ok(Weighting::Reference),
        other => Err(LawError(format!(
            "weighting `{other}` is not one of {}",
            WEIGHTINGS.join(", ")
        ))),
    }
}

// ---------------------------------------------------------------------------
// The band
// ---------------------------------------------------------------------------

/// The in-band lines of a rectangular stack, as a stack of their own.
pub(crate) fn band_matrix(m: &MatrixStack, band: (usize, usize)) -> MatrixStack {
    let (_, r, c) = m.dims();
    let per = r * c;
    MatrixStack::from_vec(
        band.1 - band.0,
        r,
        c,
        m.data()[band.0 * per..band.1 * per].to_vec(),
    )
    .expect("a slice of whole lines")
}

/// The in-band lines of a Hermitian stack, as a stack of their own.
pub(crate) fn band_hermitian(h: &HermitianStack, band: (usize, usize)) -> HermitianStack {
    let per = HermitianStack::packed_len(h.n());
    HermitianStack::from_packed(
        band.1 - band.0,
        h.n(),
        h.data()[band.0 * per..band.1 * per].to_vec(),
    )
    .expect("a slice of whole lines")
}

// ---------------------------------------------------------------------------
// The inverse
// ---------------------------------------------------------------------------

/// How the inverse of the plant is formed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum InverseRule {
    /// The regularized pseudo-inverse, with the controls weighted.
    Regularized {
        reg: Regularization,
        weighting: Weighting,
    },
    /// The `m` leading right singular directions only (shape-constrained).
    Shape(ShapeSelection),
}

/// How many leading singular directions the shape-constrained inverse keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ShapeSelection {
    /// Exactly this many (fewer where the plant's rank is lower).
    Count(usize),
    /// Those at or above `tau` times the largest, at most `max`.
    Ratio { tau: f64, max: usize },
}

/// The inverse of the plant on the in-band lines, what it kept, and the
/// singular value decomposition it was formed from.
///
/// With the controls weighted by `W` (identity without weighting) and
/// `W H = U diag(s) V^H` per line, the inverse is `H^+ = V diag(f) U^H W` with
/// filter factors `f`: `1 / s` on the kept singular values (truncated, none,
/// shape-constrained) or `s / (s^2 + mu^2)` (Tikhonov), zero on the others.
/// Every rule first drops the singular values at or below
/// `max(rows, cols) * eps * s_max`, as `hertzforge_sdm::linalg::pinv` does, and
/// for the regularized rules the result is that function's.
#[derive(Clone, Debug)]
pub(crate) struct Inverse {
    pub nc: usize,
    pub nd: usize,
    /// Singular values per line, `min(nc, nd)`.
    pub k: usize,
    /// Drives by controls per in-band line.
    pub hp: MatrixStack,
    /// Singular values used per in-band line.
    pub kept: Vec<usize>,
    /// Condition number of the (weighted) plant per in-band line.
    pub cond: Vec<f64>,
    /// The (weighted) plant's singular values, descending, `k` per line.
    pub sv: Vec<f64>,
    /// Left singular vectors, controls by `k` per line.
    pub u: MatrixStack,
    /// Right singular vectors, drives by `k` per line.
    pub v: MatrixStack,
    /// Filter factors, `k` per line (zero past the kept ones).
    pub f: Vec<f64>,
    /// Control weights, `nc` per line, when the inverse is weighted.
    pub w: Option<Vec<f64>>,
}

impl Inverse {
    /// Independent sources the plant can carry on in-band line `i`: the
    /// singular values the inverse kept that are also above `rank_tol` times
    /// the largest.
    pub fn sources(&self, i: usize, rank_tol: f64) -> usize {
        let s = &self.sv[i * self.k..(i + 1) * self.k];
        let smax = s.first().copied().unwrap_or(0.0);
        let above = s
            .iter()
            .take_while(|&&x| x > rank_tol * smax && x > 0.0)
            .count();
        above.min(self.kept[i])
    }

    /// The control weight of control `c` on in-band line `i`.
    pub fn weight(&self, i: usize, c: usize) -> f64 {
        self.w.as_ref().map(|w| w[i * self.nc + c]).unwrap_or(1.0)
    }
}

/// One over the square root of each control's reference PSD on in-band line
/// `i`, so a quiet control counts as much as a loud one. A control with no
/// reference is weighed as the line's loudest one millionth.
fn weights(reference: &HermitianStack, i: usize) -> Vec<f64> {
    let d = reference.diagonal(i);
    let gmax = d.iter().copied().fold(0.0f64, f64::max);
    d.iter()
        .map(|&g| {
            let g = g.max(1e-12 * gmax);
            if g > 0.0 {
                1.0 / g.sqrt()
            } else {
                1.0
            }
        })
        .collect()
}

/// The filter factors of one line's descending singular values `s` under
/// `rule`, into `f`; returns how many were kept.
fn filter(s: &[f64], dim_max: usize, rule: InverseRule, f: &mut [f64]) -> usize {
    let smax = s.first().copied().unwrap_or(0.0);
    let floor = dim_max as f64 * f64::EPSILON * smax;
    let rank = s.iter().take_while(|&&x| x > floor).count();
    f.fill(0.0);
    let kept = match rule {
        InverseRule::Regularized { reg, .. } => match reg {
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
                return rank;
            }
        },
        InverseRule::Shape(ShapeSelection::Count(m)) => m.min(rank),
        InverseRule::Shape(ShapeSelection::Ratio { tau, max }) => s
            .iter()
            .take_while(|&&x| x >= tau * smax)
            .count()
            .min(max)
            .min(rank),
    };
    for j in 0..kept {
        f[j] = 1.0 / s[j];
    }
    kept
}

/// The inverse of the in-band plant `h` (controls by drives) by `rule`.
/// `reference` holds the in-band reference lines (for the weighting).
///
/// The shape-constrained rule keeps the `m` leading right singular vectors
/// `C = V_m`: `H C = U_m S_m`, so `C (H C)^+ = V_m S_m^-1 U_m^H` (R. Schultz
/// and P. Avitabile, "Shape-Constrained Input Estimation for Efficient
/// Multi-shaker Vibration Testing", Experimental Techniques 44, 2020: the
/// constrained FRF, the constrained inputs and the back-transformation).
pub(crate) fn inverse(
    h: &MatrixStack,
    reference: &HermitianStack,
    rule: InverseRule,
) -> Result<Inverse, LawError> {
    let (lines, nc, nd) = h.dims();
    let w: Option<Vec<f64>> = match rule {
        InverseRule::Regularized {
            weighting: Weighting::Reference,
            ..
        } => Some((0..lines).flat_map(|i| weights(reference, i)).collect()),
        _ => None,
    };
    let svd = match &w {
        None => linalg::svd(h)?,
        Some(w) => {
            let mut hw = h.clone();
            for i in 0..lines {
                let line = hw.line_mut(i);
                for r in 0..nc {
                    for x in &mut line[r * nd..(r + 1) * nd] {
                        *x *= w[i * nc + r];
                    }
                }
            }
            linalg::svd(&hw)?
        }
    };
    let k = svd.k();
    let mut hp = MatrixStack::zeros(lines, nd, nc);
    let mut kept = vec![0; lines];
    let mut cond = vec![0.0; lines];
    let mut f = vec![0.0; lines * k];
    for i in 0..lines {
        let s = &svd.s[i * k..(i + 1) * k];
        let fi = &mut f[i * k..(i + 1) * k];
        kept[i] = filter(s, nc.max(nd), rule, fi);
        cond[i] = if s[k - 1] > 0.0 {
            s[0] / s[k - 1]
        } else {
            f64::INFINITY
        };
        let (u, v) = (svd.u.line(i), svd.v.line(i));
        let out = hp.line_mut(i);
        for j in 0..k {
            if fi[j] == 0.0 {
                continue;
            }
            for d in 0..nd {
                let vd = v[d * k + j] * fi[j];
                for c in 0..nc {
                    out[d * nc + c] += vd * u[c * k + j].conj();
                }
            }
        }
        if let Some(w) = &w {
            for d in 0..nd {
                for c in 0..nc {
                    out[d * nc + c] *= w[i * nc + c];
                }
            }
        }
    }
    Ok(Inverse {
        nc,
        nd,
        k,
        hp,
        kept,
        cond,
        sv: svd.s,
        u: svd.u,
        v: svd.v,
        f,
        w,
    })
}

/// The inverse for `frf`, kept while the plant estimate is unchanged: at full
/// scale a decomposition costs a large part of a loop, and with the FRF held
/// (the controller's default) it never changes after the seed.
#[derive(Debug, Default)]
pub(crate) struct InverseCache {
    key: Vec<C64>,
    inverse: Option<Inverse>,
    generation: u64,
}

impl InverseCache {
    /// The inverse of the in-band lines of `frf` by `rule`, recomputed only
    /// when those lines changed.
    pub fn get(
        &mut self,
        frf: &MatrixStack,
        band: (usize, usize),
        reference: &HermitianStack,
        rule: InverseRule,
    ) -> Result<&Inverse, LawError> {
        let (_, r, c) = frf.dims();
        let lines = &frf.data()[band.0 * r * c..band.1 * r * c];
        let fresh = match &self.inverse {
            Some(_) => self.key.as_slice() != lines,
            None => true,
        };
        if fresh {
            let inv = inverse(&band_matrix(frf, band), reference, rule)?;
            self.key = lines.to_vec();
            self.inverse = Some(inv);
            self.generation += 1;
        }
        Ok(self.inverse.as_ref().expect("filled above"))
    }

    /// The inverse last formed, if any.
    pub fn current(&self) -> Option<&Inverse> {
        self.inverse.as_ref()
    }

    /// How many times the inverse has been formed: a change means the plant
    /// estimate changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

// ---------------------------------------------------------------------------
// Per-line products
// ---------------------------------------------------------------------------

/// `B B^H` of `b` (`n` by `m`, row-major), packed into `out` (upper triangle,
/// row-major). A Gram matrix: positive semi-definite by construction.
pub(crate) fn gram_packed(b: &[C64], n: usize, m: usize, out: &mut [C64]) {
    let mut k = 0;
    for r in 0..n {
        for c in r..n {
            let mut s = ZERO;
            for t in 0..m {
                s += b[r * m + t] * b[c * m + t].conj();
            }
            out[k] = if r == c { C64::new(s.re, 0.0) } else { s };
            k += 1;
        }
    }
}

/// `a` (`r` by `k`) times `b` (`k` by `c`), row-major, into `out`.
pub(crate) fn matmul_into(a: &[C64], b: &[C64], r: usize, k: usize, c: usize, out: &mut [C64]) {
    for i in 0..r {
        for j in 0..c {
            let mut s = ZERO;
            for t in 0..k {
                s += a[i * k + t] * b[t * c + j];
            }
            out[i * c + j] = s;
        }
    }
}

/// `a` (`r` by `k`) times the conjugate transpose of `b` (`c` by `k`),
/// row-major, into `out` (`r` by `c`).
pub(crate) fn matmul_adj_into(a: &[C64], b: &[C64], r: usize, k: usize, c: usize, out: &mut [C64]) {
    for i in 0..r {
        for j in 0..c {
            let mut s = ZERO;
            for t in 0..k {
                s += a[i * k + t] * b[j * k + t].conj();
            }
            out[i * c + j] = s;
        }
    }
}

/// The diagonals of `B B^H`: the squared norm of each row of `b`.
pub(crate) fn row_power(b: &[C64], n: usize, m: usize, out: &mut [f64]) {
    for r in 0..n {
        out[r] = b[r * m..(r + 1) * m].iter().map(|v| v.norm_sqr()).sum();
    }
}

/// The largest change of any drive PSD, in dB, between `old` and `new` over the
/// band. A PSD is measured against a floor 60 dB below its line's loudest, so a
/// drive rising from nothing reads as a finite change.
pub(crate) fn largest_change_db(
    old: &HermitianStack,
    new: &HermitianStack,
    band: (usize, usize),
) -> f64 {
    let mut worst: f64 = 0.0;
    for line in band.0..band.1 {
        let (a, b) = (old.diagonal(line), new.diagonal(line));
        let top = a.iter().chain(&b).copied().fold(0.0f64, f64::max);
        if top <= 0.0 {
            continue;
        }
        let floor = 1e-6 * top;
        for (x, y) in a.iter().zip(&b) {
            let db = 10.0 * (y.max(floor) / x.max(floor)).log10();
            worst = worst.max(db.abs());
        }
    }
    worst
}

/// The per-line diagnostics of an inverse over the whole grid: condition
/// numbers (0 out of band, [`SINGULAR_CONDITION`] for a singular line) and
/// singular values kept.
pub(crate) fn inverse_diagnostics(
    inv: &Inverse,
    lines: usize,
    band: (usize, usize),
) -> (Vec<f64>, Vec<f64>) {
    let mut cond = vec![0.0; lines];
    let mut kept = vec![0.0; lines];
    for (i, line) in (band.0..band.1).enumerate() {
        cond[line] = if inv.cond[i].is_finite() {
            inv.cond[i]
        } else {
            SINGULAR_CONDITION
        };
        kept[line] = inv.kept[i] as f64;
    }
    (cond, kept)
}
