//! Small dense complex matrices, one line at a time.
//!
//! The fill rule, the band statements and the band laws work on one line's
//! `n` by `n` (or `n` by `m`) matrix at a time, at loop rate, off the
//! real-time thread. [`CMat`] is that matrix, row-major; Hermitian functions
//! (square root, inverse square root, logarithm, exponential) go through the
//! SDM crate's eigendecomposition ([`hertzforge_sdm::linalg::eigh`]) and the
//! pseudo-inverse through its regularized SVD ([`hertzforge_sdm::linalg::pinv`]).
//! These allocate; nothing here is called per frame.

use hertzforge_sdm::linalg::{self, LinalgError, Regularization};
use hertzforge_sdm::{HermitianStack, MatrixStack, C64};

/// Regularization of the model's inverse `Z` wherever this crate forms one:
/// a truncated SVD keeping singular values above a thousandth of the largest,
/// which is MIMO Random's default condition gate (`Gates::condition_threshold`
/// 1000) read as a truncation.
pub const Z_REGULARIZATION: Regularization = Regularization::Truncated { rcond: 1e-3 };

/// A dense complex matrix, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct CMat {
    rows: usize,
    cols: usize,
    data: Vec<C64>,
}

impl CMat {
    /// All zeros.
    pub fn zeros(rows: usize, cols: usize) -> Self {
        CMat {
            rows,
            cols,
            data: vec![C64::new(0.0, 0.0); rows * cols],
        }
    }

    /// The identity, `n` by `n`.
    pub fn identity(n: usize) -> Self {
        let mut m = CMat::zeros(n, n);
        for i in 0..n {
            m.set(i, i, C64::new(1.0, 0.0));
        }
        m
    }

    /// A real diagonal matrix.
    pub fn diag(d: &[f64]) -> Self {
        let mut m = CMat::zeros(d.len(), d.len());
        for (i, &v) in d.iter().enumerate() {
            m.set(i, i, C64::new(v, 0.0));
        }
        m
    }

    /// From row-major values.
    pub fn from_rows(rows: usize, cols: usize, data: Vec<C64>) -> Self {
        assert_eq!(data.len(), rows * cols, "CMat::from_rows: shape");
        CMat { rows, cols, data }
    }

    /// One line of a Hermitian stack, in full.
    pub fn from_hermitian(g: &HermitianStack, line: usize) -> Self {
        let n = g.n();
        let mut m = CMat::zeros(n, n);
        for r in 0..n {
            for c in 0..n {
                m.set(r, c, g.get(line, r, c));
            }
        }
        m
    }

    /// One line of a rectangular stack.
    pub fn from_stack(h: &MatrixStack, line: usize) -> Self {
        CMat {
            rows: h.rows(),
            cols: h.cols(),
            data: h.line(line).to_vec(),
        }
    }

    /// Rows.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Columns.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// The values, row-major.
    pub fn data(&self) -> &[C64] {
        &self.data
    }

    /// Entry `(r, c)`.
    #[inline]
    pub fn get(&self, r: usize, c: usize) -> C64 {
        self.data[r * self.cols + c]
    }

    /// Set entry `(r, c)`.
    #[inline]
    pub fn set(&mut self, r: usize, c: usize, v: C64) {
        self.data[r * self.cols + c] = v;
    }

    /// The conjugate transpose.
    pub fn adjoint(&self) -> CMat {
        let mut m = CMat::zeros(self.cols, self.rows);
        for r in 0..self.rows {
            for c in 0..self.cols {
                m.set(c, r, self.get(r, c).conj());
            }
        }
        m
    }

    /// `self * other`.
    pub fn mul(&self, other: &CMat) -> CMat {
        assert_eq!(self.cols, other.rows, "CMat::mul: inner dimensions");
        let mut m = CMat::zeros(self.rows, other.cols);
        for r in 0..self.rows {
            for k in 0..self.cols {
                let a = self.get(r, k);
                if a == C64::new(0.0, 0.0) {
                    continue;
                }
                for c in 0..other.cols {
                    m.data[r * other.cols + c] += a * other.get(k, c);
                }
            }
        }
        m
    }

    /// `self * v`.
    pub fn mul_vec(&self, v: &[C64]) -> Vec<C64> {
        assert_eq!(self.cols, v.len(), "CMat::mul_vec: dimensions");
        (0..self.rows)
            .map(|r| (0..self.cols).map(|c| self.get(r, c) * v[c]).sum())
            .collect()
    }

    /// `self + other`.
    pub fn add(&self, other: &CMat) -> CMat {
        assert_eq!((self.rows, self.cols), (other.rows, other.cols));
        CMat {
            rows: self.rows,
            cols: self.cols,
            data: self
                .data
                .iter()
                .zip(&other.data)
                .map(|(a, b)| a + b)
                .collect(),
        }
    }

    /// `self - other`.
    pub fn sub(&self, other: &CMat) -> CMat {
        assert_eq!((self.rows, self.cols), (other.rows, other.cols));
        CMat {
            rows: self.rows,
            cols: self.cols,
            data: self
                .data
                .iter()
                .zip(&other.data)
                .map(|(a, b)| a - b)
                .collect(),
        }
    }

    /// `k * self`.
    pub fn scaled(&self, k: f64) -> CMat {
        CMat {
            rows: self.rows,
            cols: self.cols,
            data: self.data.iter().map(|a| a * k).collect(),
        }
    }

    /// `T self T^H`.
    pub fn congruence(&self, t: &CMat) -> CMat {
        t.mul(self).mul(&t.adjoint()).hermitian_part()
    }

    /// `(self + self^H) / 2`.
    pub fn hermitian_part(&self) -> CMat {
        assert_eq!(self.rows, self.cols, "hermitian_part: square");
        let n = self.rows;
        let mut m = CMat::zeros(n, n);
        for r in 0..n {
            for c in 0..n {
                m.set(r, c, (self.get(r, c) + self.get(c, r).conj()) * 0.5);
            }
        }
        m
    }

    /// The largest entry modulus.
    pub fn max_abs(&self) -> f64 {
        self.data.iter().map(|v| v.norm()).fold(0.0, f64::max)
    }

    /// The Frobenius norm.
    pub fn norm(&self) -> f64 {
        self.data.iter().map(|v| v.norm_sqr()).sum::<f64>().sqrt()
    }

    /// Whether every entry is finite.
    pub fn is_finite(&self) -> bool {
        self.data
            .iter()
            .all(|v| v.re.is_finite() && v.im.is_finite())
    }

    /// Write a Hermitian matrix into line `line` of a stack (its Hermitian
    /// part; the upper triangle is what the stack keeps).
    pub fn store_hermitian(&self, g: &mut HermitianStack, line: usize) {
        let n = g.n();
        assert_eq!((self.rows, self.cols), (n, n), "store_hermitian: shape");
        for r in 0..n {
            for c in r..n {
                let v = if r == c {
                    C64::new(self.get(r, r).re, 0.0)
                } else {
                    (self.get(r, c) + self.get(c, r).conj()) * 0.5
                };
                g.set(line, r, c, v);
            }
        }
    }

    /// Write into line `line` of a rectangular stack.
    pub fn store(&self, h: &mut MatrixStack, line: usize) {
        assert_eq!((self.rows, self.cols), (h.rows(), h.cols()), "store: shape");
        h.line_mut(line).copy_from_slice(&self.data);
    }

    /// Eigenvalues (ascending) and eigenvectors (columns) of the Hermitian
    /// part.
    pub fn eigh(&self) -> Result<(Vec<f64>, CMat), LinalgError> {
        let n = self.rows;
        let mut g = HermitianStack::zeros(1, n);
        self.store_hermitian(&mut g, 0);
        let e = linalg::eigh(&g)?;
        Ok((e.values, CMat::from_stack(&e.vectors, 0)))
    }

    /// `f` applied to the eigenvalues of the Hermitian part:
    /// `V diag(f(lambda)) V^H`.
    pub fn hermitian_fn(&self, f: impl Fn(f64) -> f64) -> Result<CMat, LinalgError> {
        let (vals, v) = self.eigh()?;
        let n = self.rows;
        let mut m = CMat::zeros(n, n);
        for (k, &lam) in vals.iter().enumerate() {
            let fl = f(lam);
            for r in 0..n {
                let a = v.get(r, k) * fl;
                for c in 0..n {
                    m.data[r * n + c] += a * v.get(c, k).conj();
                }
            }
        }
        Ok(m.hermitian_part())
    }

    /// Smallest and largest eigenvalue of the Hermitian part.
    pub fn eig_range(&self) -> Result<(f64, f64), LinalgError> {
        let (vals, _) = self.eigh()?;
        Ok((
            vals.first().copied().unwrap_or(0.0),
            vals.last().copied().unwrap_or(0.0),
        ))
    }

    /// Largest over smallest eigenvalue of a Hermitian matrix; infinite when
    /// the smallest is at or below zero.
    pub fn condition(&self) -> Result<f64, LinalgError> {
        let (lo, hi) = self.eig_range()?;
        Ok(if lo > 0.0 { hi / lo } else { f64::INFINITY })
    }

    /// The regularized pseudo-inverse under [`Z_REGULARIZATION`] (or `reg`).
    pub fn pinv(&self, reg: Regularization) -> Result<CMat, LinalgError> {
        let h = MatrixStack::from_vec(1, self.rows, self.cols, self.data.clone())
            .map_err(LinalgError::from)?;
        let (z, _) = linalg::pinv(&h, reg)?;
        Ok(CMat::from_stack(&z, 0))
    }
}

/// The geometric mean of `inv(a)` and `b` for Hermitian positive definite
/// `a` and `b`: the positive definite `X` with `X a X = b`,
/// `X = a^-1/2 (a^1/2 b a^1/2)^1/2 a^-1/2` (Derivations 2.6, C3).
pub fn inv_geometric_mean(a: &CMat, b: &CMat) -> Result<CMat, LinalgError> {
    let ah = a.hermitian_fn(|l| l.max(0.0).sqrt())?;
    let aih = a.hermitian_fn(|l| if l > 0.0 { 1.0 / l.sqrt() } else { 0.0 })?;
    let mid = b.congruence(&ah);
    let root = mid.hermitian_fn(|l| l.max(0.0).sqrt())?;
    Ok(root.congruence(&aih))
}

/// The Hermitian logarithm of a positive definite matrix.
pub fn logm(x: &CMat) -> Result<CMat, LinalgError> {
    x.hermitian_fn(|l| l.max(f64::MIN_POSITIVE).ln())
}

/// The Hermitian exponential.
pub fn expm(x: &CMat) -> Result<CMat, LinalgError> {
    x.hermitian_fn(f64::exp)
}

/// Log-Euclidean interpolation between positive definite `a` (at `t = 0`)
/// and `b` (at `t = 1`): `exp((1 - t) log a + t log b)`.
pub fn log_euclidean(a: &CMat, b: &CMat, t: f64) -> Result<CMat, LinalgError> {
    let la = logm(a)?;
    let lb = logm(b)?;
    expm(&la.scaled(1.0 - t).add(&lb.scaled(t)))
}

/// The model `H` on a grid at frequency `f_hz`, linear between the two lines
/// either side (clamped at the grid's ends).
pub fn model_at(h: &MatrixStack, df_hz: f64, f_hz: f64) -> CMat {
    let x = (f_hz / df_hz).max(0.0);
    let last = h.lines().saturating_sub(1);
    let a = (x.floor() as usize).min(last);
    let b = (a + 1).min(last);
    let t = (x - a as f64).clamp(0.0, 1.0);
    let ha = CMat::from_stack(h, a);
    if a == b || t == 0.0 {
        return ha;
    }
    let hb = CMat::from_stack(h, b);
    ha.scaled(1.0 - t).add(&hb.scaled(t))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(re: f64, im: f64) -> C64 {
        C64::new(re, im)
    }

    #[test]
    fn mimo_mixed_geometric_mean_solves_x_a_x_equals_b() {
        let a = CMat::from_rows(
            2,
            2,
            vec![c(2.0, 0.0), c(0.3, 0.4), c(0.3, -0.4), c(1.0, 0.0)],
        );
        let b = CMat::from_rows(
            2,
            2,
            vec![c(1.5, 0.0), c(-0.2, 0.1), c(-0.2, -0.1), c(0.7, 0.0)],
        );
        let x = inv_geometric_mean(&a, &b).unwrap();
        let back = a.congruence(&x);
        assert!(back.sub(&b).max_abs() < 1e-12, "{back:?}");
        let (lo, _) = x.eig_range().unwrap();
        assert!(lo > 0.0);
    }

    #[test]
    fn mimo_mixed_log_and_exp_invert_each_other() {
        let a = CMat::from_rows(
            2,
            2,
            vec![c(2.0, 0.0), c(0.3, 0.4), c(0.3, -0.4), c(1.0, 0.0)],
        );
        let back = expm(&logm(&a).unwrap()).unwrap();
        assert!(back.sub(&a).max_abs() < 1e-12);
        let mid = log_euclidean(&a, &a, 0.37).unwrap();
        assert!(mid.sub(&a).max_abs() < 1e-12);
    }
}
