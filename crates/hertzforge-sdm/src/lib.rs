//! `hertzforge-sdm`: spectral density matrices on a line grid, and the boundary a
//! multiple-input, multiple-output (MIMO) control law is written against.
//!
//! # Conventions (fixed; see the MIMO Random design, section 7.0)
//!
//! - A spectral matrix is `G = lim (1/T) E[X X^H]` per line, so a response `Y = H X`
//!   has `G_yy = H G_xx H^H`. This is the complex conjugate of the Bendat and Piersol
//!   definition. Every importer and every conversion at the service's edge
//!   conjugates where the source convention differs.
//! - A [`MatrixStack`] holds one `rows` by `cols` complex matrix per line, row-major
//!   within a line, lines in order: `data[(line * rows + r) * cols + c]`.
//! - A [`HermitianStack`] holds one `n` by `n` Hermitian matrix per line as its
//!   packed upper triangle, row-major: for `r <= c`, `data[line * packed_len + r * n
//!   - r * (r - 1) / 2 + (c - r)]`. An element below the diagonal is the conjugate of
//!   its mirror. Diagonals are real.
//! - Units: a response SDM is in the channel's units squared per hertz; a drive SDM in
//!   volts squared per hertz; an FRF in response units per volt.
//!
//! This crate carries types and shape checks only. Linear algebra (factorizations,
//! pseudo-inverses, repairs) lives in the crates built on top of it.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::fmt;

pub use num_complex::Complex;

/// A complex double.
pub type C64 = Complex<f64>;

pub mod law;

/// A dimension mismatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeError(pub String);

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ShapeError {}

/// One complex matrix per line, stacked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatrixStack {
    lines: usize,
    rows: usize,
    cols: usize,
    data: Vec<C64>,
}

impl MatrixStack {
    /// All zeros.
    pub fn zeros(lines: usize, rows: usize, cols: usize) -> Self {
        MatrixStack {
            lines,
            rows,
            cols,
            data: vec![C64::new(0.0, 0.0); lines * rows * cols],
        }
    }

    /// From a flat vector in this crate's layout. The length must be
    /// `lines * rows * cols`.
    pub fn from_vec(
        lines: usize,
        rows: usize,
        cols: usize,
        data: Vec<C64>,
    ) -> Result<Self, ShapeError> {
        if data.len() != lines * rows * cols {
            return Err(ShapeError(format!(
                "matrix stack of {lines} lines by {rows} x {cols} needs {} values, got {}",
                lines * rows * cols,
                data.len()
            )));
        }
        Ok(MatrixStack {
            lines,
            rows,
            cols,
            data,
        })
    }

    /// Identity on every line (square only).
    pub fn identity(lines: usize, n: usize) -> Self {
        let mut m = Self::zeros(lines, n, n);
        for line in 0..lines {
            for i in 0..n {
                m.set(line, i, i, C64::new(1.0, 0.0));
            }
        }
        m
    }

    pub fn lines(&self) -> usize {
        self.lines
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    /// `(lines, rows, cols)`.
    pub fn dims(&self) -> (usize, usize, usize) {
        (self.lines, self.rows, self.cols)
    }

    pub fn data(&self) -> &[C64] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [C64] {
        &mut self.data
    }

    pub fn into_data(self) -> Vec<C64> {
        self.data
    }

    #[inline]
    fn index(&self, line: usize, r: usize, c: usize) -> usize {
        debug_assert!(line < self.lines && r < self.rows && c < self.cols);
        (line * self.rows + r) * self.cols + c
    }

    #[inline]
    pub fn get(&self, line: usize, r: usize, c: usize) -> C64 {
        self.data[self.index(line, r, c)]
    }

    #[inline]
    pub fn set(&mut self, line: usize, r: usize, c: usize, v: C64) {
        let i = self.index(line, r, c);
        self.data[i] = v;
    }

    /// One line's matrix, row-major.
    pub fn line(&self, line: usize) -> &[C64] {
        let n = self.rows * self.cols;
        &self.data[line * n..(line + 1) * n]
    }

    pub fn line_mut(&mut self, line: usize) -> &mut [C64] {
        let n = self.rows * self.cols;
        &mut self.data[line * n..(line + 1) * n]
    }

    /// The conjugate transpose of every line: a `cols` by `rows` stack.
    pub fn conj_transposed(&self) -> MatrixStack {
        let mut out = MatrixStack::zeros(self.lines, self.cols, self.rows);
        for line in 0..self.lines {
            for r in 0..self.rows {
                for c in 0..self.cols {
                    out.set(line, c, r, self.get(line, r, c).conj());
                }
            }
        }
        out
    }

    /// True when every value is finite.
    pub fn is_finite(&self) -> bool {
        self.data
            .iter()
            .all(|v| v.re.is_finite() && v.im.is_finite())
    }

    /// Relative Hermitian error of one line, `||G - G^H||_F / ||G||_F`
    /// (0 for an all-zero line). Square stacks only.
    pub fn hermitian_error(&self, line: usize) -> Result<f64, ShapeError> {
        if self.rows != self.cols {
            return Err(ShapeError(format!(
                "hermitian error needs a square matrix, got {} x {}",
                self.rows, self.cols
            )));
        }
        let n = self.rows;
        let mut num = 0.0;
        let mut den = 0.0;
        for r in 0..n {
            for c in 0..n {
                let a = self.get(line, r, c);
                let b = self.get(line, c, r).conj();
                num += (a - b).norm_sqr();
                den += a.norm_sqr();
            }
        }
        Ok(if den == 0.0 { 0.0 } else { (num / den).sqrt() })
    }

    /// The largest relative Hermitian error over all lines.
    pub fn max_hermitian_error(&self) -> Result<f64, ShapeError> {
        let mut worst = 0.0f64;
        for line in 0..self.lines {
            worst = worst.max(self.hermitian_error(line)?);
        }
        Ok(worst)
    }
}

/// One Hermitian matrix per line, packed (upper triangle, row-major).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HermitianStack {
    lines: usize,
    n: usize,
    data: Vec<C64>,
}

impl HermitianStack {
    /// Packed values per line for an `n` by `n` matrix.
    pub const fn packed_len(n: usize) -> usize {
        n * (n + 1) / 2
    }

    /// Offset of `(r, c)` with `r <= c` inside one packed line.
    #[inline]
    pub fn packed_index(n: usize, r: usize, c: usize) -> usize {
        debug_assert!(r <= c && c < n);
        // Row `r` starts after rows 0..r, which hold n, n-1, ..., n-r+1 values:
        // r * (2n - r + 1) / 2 in all.
        r * (2 * n - r + 1) / 2 + (c - r)
    }

    /// All zeros.
    pub fn zeros(lines: usize, n: usize) -> Self {
        HermitianStack {
            lines,
            n,
            data: vec![C64::new(0.0, 0.0); lines * Self::packed_len(n)],
        }
    }

    /// From packed data. The length must be `lines * packed_len(n)`. Diagonal
    /// imaginary parts are forced to zero.
    pub fn from_packed(lines: usize, n: usize, mut data: Vec<C64>) -> Result<Self, ShapeError> {
        let want = lines * Self::packed_len(n);
        if data.len() != want {
            return Err(ShapeError(format!(
                "packed Hermitian stack of {lines} lines by {n} needs {want} values, got {}",
                data.len()
            )));
        }
        let per = Self::packed_len(n);
        for line in 0..lines {
            for i in 0..n {
                let k = line * per + Self::packed_index(n, i, i);
                data[k].im = 0.0;
            }
        }
        Ok(HermitianStack { lines, n, data })
    }

    /// From a full square stack, taking the Hermitian part `(G + G^H) / 2`. Use
    /// [`HermitianStack::from_full_checked`] when a non-Hermitian input must be
    /// refused rather than averaged.
    pub fn from_full(full: &MatrixStack) -> Result<Self, ShapeError> {
        if full.rows != full.cols {
            return Err(ShapeError(format!(
                "a Hermitian stack needs a square matrix, got {} x {}",
                full.rows, full.cols
            )));
        }
        let n = full.rows;
        let mut out = Self::zeros(full.lines, n);
        for line in 0..full.lines {
            for r in 0..n {
                for c in r..n {
                    let v = (full.get(line, r, c) + full.get(line, c, r).conj()) * 0.5;
                    out.set(line, r, c, v);
                }
            }
        }
        Ok(out)
    }

    /// From a full square stack whose every line is Hermitian within `rel_tol`
    /// (relative Frobenius error). Refuses otherwise, naming the worst line.
    pub fn from_full_checked(full: &MatrixStack, rel_tol: f64) -> Result<Self, ShapeError> {
        let mut worst = (0usize, 0.0f64);
        for line in 0..full.lines {
            let e = full.hermitian_error(line)?;
            if e > worst.1 {
                worst = (line, e);
            }
        }
        if worst.1 > rel_tol {
            return Err(ShapeError(format!(
                "line {} is not Hermitian: relative error {:.3e} exceeds {:.3e}",
                worst.0, worst.1, rel_tol
            )));
        }
        Self::from_full(full)
    }

    /// The full `n` by `n` stack, both triangles filled.
    pub fn to_full(&self) -> MatrixStack {
        let mut out = MatrixStack::zeros(self.lines, self.n, self.n);
        for line in 0..self.lines {
            for r in 0..self.n {
                for c in r..self.n {
                    let v = self.get(line, r, c);
                    out.set(line, r, c, v);
                    out.set(line, c, r, v.conj());
                }
            }
        }
        out
    }

    pub fn lines(&self) -> usize {
        self.lines
    }

    /// The matrix order `n`.
    pub fn n(&self) -> usize {
        self.n
    }

    /// Packed data, all lines.
    pub fn data(&self) -> &[C64] {
        &self.data
    }

    pub fn into_data(self) -> Vec<C64> {
        self.data
    }

    /// One line's packed values.
    pub fn line(&self, line: usize) -> &[C64] {
        let per = Self::packed_len(self.n);
        &self.data[line * per..(line + 1) * per]
    }

    /// Element `(r, c)`, either triangle.
    #[inline]
    pub fn get(&self, line: usize, r: usize, c: usize) -> C64 {
        let per = Self::packed_len(self.n);
        if r <= c {
            self.data[line * per + Self::packed_index(self.n, r, c)]
        } else {
            self.data[line * per + Self::packed_index(self.n, c, r)].conj()
        }
    }

    /// Set element `(r, c)`; setting the lower triangle stores the conjugate. A
    /// diagonal element's imaginary part is dropped.
    #[inline]
    pub fn set(&mut self, line: usize, r: usize, c: usize, v: C64) {
        let per = Self::packed_len(self.n);
        if r == c {
            let k = line * per + Self::packed_index(self.n, r, c);
            self.data[k] = C64::new(v.re, 0.0);
        } else if r < c {
            let k = line * per + Self::packed_index(self.n, r, c);
            self.data[k] = v;
        } else {
            let k = line * per + Self::packed_index(self.n, c, r);
            self.data[k] = v.conj();
        }
    }

    /// The real diagonal of one line (the power spectral densities).
    pub fn diagonal(&self, line: usize) -> Vec<f64> {
        (0..self.n).map(|i| self.get(line, i, i).re).collect()
    }

    /// The trace of one line.
    pub fn trace(&self, line: usize) -> f64 {
        self.diagonal(line).iter().sum()
    }

    /// True when every value is finite.
    pub fn is_finite(&self) -> bool {
        self.data
            .iter()
            .all(|v| v.re.is_finite() && v.im.is_finite())
    }

    /// Scale every line's matrix by a real factor per line (a level change keeps
    /// coherence and phase).
    pub fn scale_lines(&mut self, factors: &[f64]) -> Result<(), ShapeError> {
        if factors.len() != self.lines {
            return Err(ShapeError(format!(
                "{} line factors for {} lines",
                factors.len(),
                self.lines
            )));
        }
        let per = Self::packed_len(self.n);
        for (line, f) in factors.iter().enumerate() {
            for v in &mut self.data[line * per..(line + 1) * per] {
                *v *= *f;
            }
        }
        Ok(())
    }

    /// `D^(1/2) G D^(1/2)` per line with `D` the given non-negative diagonal: a
    /// congruence that preserves positive semi-definiteness and every pair's
    /// coherence. `scales` holds `d_i` per channel for every line, line-major
    /// (`lines * n` values).
    pub fn diagonal_congruence(&mut self, scales: &[f64]) -> Result<(), ShapeError> {
        if scales.len() != self.lines * self.n {
            return Err(ShapeError(format!(
                "{} diagonal scales for {} lines by {} channels",
                scales.len(),
                self.lines,
                self.n
            )));
        }
        for line in 0..self.lines {
            let s = &scales[line * self.n..(line + 1) * self.n];
            for r in 0..self.n {
                for c in r..self.n {
                    let f = (s[r].max(0.0) * s[c].max(0.0)).sqrt();
                    let v = self.get(line, r, c) * f;
                    self.set(line, r, c, v);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(re: f64, im: f64) -> C64 {
        C64::new(re, im)
    }

    #[test]
    fn packed_index_walks_the_upper_triangle_row_by_row() {
        let n = 4;
        let mut expect = 0;
        for r in 0..n {
            for col in r..n {
                assert_eq!(
                    HermitianStack::packed_index(n, r, col),
                    expect,
                    "({r},{col})"
                );
                expect += 1;
            }
        }
        assert_eq!(expect, HermitianStack::packed_len(n));
    }

    #[test]
    fn full_round_trips_through_packed_with_conjugate_mirror() {
        let mut full = MatrixStack::zeros(2, 3, 3);
        for line in 0..2 {
            for r in 0..3 {
                for col in 0..3 {
                    let v = if r == col {
                        c(1.0 + r as f64 + line as f64, 0.0)
                    } else if r < col {
                        c(
                            0.1 * (r + col) as f64,
                            0.2 * (col as f64 - r as f64) + line as f64,
                        )
                    } else {
                        c(
                            0.1 * (r + col) as f64,
                            -(0.2 * (r as f64 - col as f64) + line as f64),
                        )
                    };
                    full.set(line, r, col, v);
                }
            }
        }
        assert!(full.max_hermitian_error().unwrap() < 1e-15);
        let packed = HermitianStack::from_full_checked(&full, 1e-12).unwrap();
        assert_eq!(packed.to_full(), full);
        assert_eq!(packed.get(1, 2, 0), full.get(1, 2, 0));
        assert_eq!(packed.get(1, 2, 0), packed.get(1, 0, 2).conj());
        assert_eq!(packed.diagonal(1), vec![2.0, 3.0, 4.0]);
        assert_eq!(packed.trace(0), 6.0);
    }

    #[test]
    fn a_non_hermitian_line_is_named_and_refused() {
        let mut full = MatrixStack::identity(3, 2);
        full.set(1, 0, 1, c(0.5, 0.0));
        full.set(1, 1, 0, c(0.0, 0.0));
        let e = full.hermitian_error(1).unwrap();
        assert!(e > 0.3, "{e}");
        let err = HermitianStack::from_full_checked(&full, 1e-9).unwrap_err();
        assert!(err.0.contains("line 1"), "{err}");
        let averaged = HermitianStack::from_full(&full).unwrap();
        assert_eq!(averaged.get(1, 0, 1), c(0.25, 0.0));
        assert_eq!(averaged.get(1, 1, 0), c(0.25, 0.0));
    }

    #[test]
    fn setting_the_lower_triangle_stores_the_conjugate_and_diagonals_stay_real() {
        let mut h = HermitianStack::zeros(1, 2);
        h.set(0, 1, 0, c(1.0, 2.0));
        assert_eq!(h.get(0, 0, 1), c(1.0, -2.0));
        assert_eq!(h.get(0, 1, 0), c(1.0, 2.0));
        h.set(0, 1, 1, c(3.0, 4.0));
        assert_eq!(h.get(0, 1, 1), c(3.0, 0.0));
    }

    #[test]
    fn wrong_lengths_are_shape_errors() {
        assert!(MatrixStack::from_vec(2, 2, 2, vec![c(0.0, 0.0); 7]).is_err());
        assert!(HermitianStack::from_packed(2, 3, vec![c(0.0, 0.0); 11]).is_err());
        assert!(MatrixStack::zeros(1, 2, 3).hermitian_error(0).is_err());
    }

    #[test]
    fn conjugate_transpose_swaps_dims_and_conjugates() {
        let mut m = MatrixStack::zeros(1, 2, 3);
        m.set(0, 1, 2, c(1.0, 1.0));
        let t = m.conj_transposed();
        assert_eq!(t.dims(), (1, 3, 2));
        assert_eq!(t.get(0, 2, 1), c(1.0, -1.0));
    }

    #[test]
    fn diagonal_congruence_scales_psds_and_keeps_coherence() {
        let mut h = HermitianStack::zeros(1, 2);
        h.set(0, 0, 0, c(4.0, 0.0));
        h.set(0, 1, 1, c(9.0, 0.0));
        h.set(0, 0, 1, c(3.0, 3.0)); // |G01|^2 / (G00 G11) = 18/36 = 0.5
        let coh = |h: &HermitianStack| {
            h.get(0, 0, 1).norm_sqr() / (h.get(0, 0, 0).re * h.get(0, 1, 1).re)
        };
        let before = coh(&h);
        h.diagonal_congruence(&[0.25, 4.0]).unwrap();
        assert!((h.get(0, 0, 0).re - 1.0).abs() < 1e-12);
        assert!((h.get(0, 1, 1).re - 36.0).abs() < 1e-12);
        assert!((coh(&h) - before).abs() < 1e-12);
    }
}
