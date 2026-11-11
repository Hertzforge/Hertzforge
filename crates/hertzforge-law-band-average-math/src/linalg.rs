//! The two factorizations the law needs, a Hermitian eigendecomposition and a thin
//! SVD of one matrix, on `hertzforge-sdm`'s (faer), in the layout the law's
//! arithmetic is written in: flat and row-major, largest value first.

use hertzforge_sdm::{linalg, HermitianStack, MatrixStack, C64};

/// A Hermitian eigendecomposition: `a = v diag(values) v^H`.
pub struct HermitianEig {
    /// The eigenvalues, largest first.
    pub values: Vec<f64>,
    /// The eigenvectors as columns, row-major, in the order of `values`.
    pub vectors: Vec<C64>,
}

/// The eigendecomposition of the `n` by `n` Hermitian matrix `a`. Only the upper
/// triangle is read. A matrix that cannot be decomposed (a value that is not
/// finite) gives NaN throughout, which the law's own checks then refuse.
pub fn hermitian_eig(a: &[C64], n: usize) -> HermitianEig {
    let mut g = HermitianStack::zeros(1, n);
    for r in 0..n {
        g.set(0, r, r, C64::new(a[r * n + r].re, 0.0));
        for c in (r + 1)..n {
            g.set(0, r, c, a[r * n + c]);
        }
    }
    match linalg::eigh(&g) {
        Ok(e) => {
            let ascending = e.vectors.line(0);
            let mut vectors = vec![C64::ZERO; n * n];
            for r in 0..n {
                for c in 0..n {
                    vectors[r * n + c] = ascending[r * n + (n - 1 - c)];
                }
            }
            HermitianEig {
                values: e.values.iter().rev().copied().collect(),
                vectors,
            }
        }
        Err(_) => HermitianEig {
            values: vec![f64::NAN; n],
            vectors: vec![C64::new(f64::NAN, f64::NAN); n * n],
        },
    }
}

/// A thin singular value decomposition: `a = u diag(s) v^H`.
pub struct Svd {
    /// `m` by `k`, columns in the order of `s`, row-major.
    pub u: Vec<C64>,
    /// The `k = min(m, n)` singular values, largest first.
    pub s: Vec<f64>,
    /// `n` by `k`, columns in the order of `s`, row-major.
    pub v: Vec<C64>,
}

/// The thin SVD of the `m` by `n` matrix `a`; NaN throughout where it cannot be
/// decomposed.
pub fn svd(a: &[C64], m: usize, n: usize) -> Svd {
    let k = m.min(n);
    let decomposed = MatrixStack::from_vec(1, m, n, a.to_vec())
        .ok()
        .and_then(|h| linalg::svd(&h).ok());
    match decomposed {
        Some(d) => Svd {
            u: d.u.line(0).to_vec(),
            s: d.s,
            v: d.v.line(0).to_vec(),
        },
        None => Svd {
            u: vec![C64::new(f64::NAN, f64::NAN); m * k],
            s: vec![f64::NAN; k],
            v: vec![C64::new(f64::NAN, f64::NAN); n * k],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix(m: usize, n: usize) -> Vec<C64> {
        (0..m * n)
            .map(|k| {
                let t = k as f64;
                C64::new((0.7 * t).sin() + 0.1 * t, (1.3 * t).cos())
            })
            .collect()
    }

    /// `u diag(s) v^H` is the matrix, wide and tall, largest value first.
    #[test]
    fn the_svd_rebuilds_the_matrix() {
        for (m, n) in [(3, 5), (5, 3), (4, 4)] {
            let a = matrix(m, n);
            let d = svd(&a, m, n);
            let k = m.min(n);
            assert!(d.s.windows(2).all(|w| w[0] >= w[1]));
            for r in 0..m {
                for c in 0..n {
                    let x: C64 = (0..k)
                        .map(|j| d.u[r * k + j] * d.s[j] * d.v[c * k + j].conj())
                        .sum();
                    assert!((x - a[r * n + c]).norm() < 1e-12, "{m}x{n} at {r},{c}");
                }
            }
        }
    }

    /// `v diag(values) v^H` is the matrix, read from its upper triangle alone.
    #[test]
    fn the_eigendecomposition_rebuilds_the_matrix_from_its_upper_triangle() {
        let n = 4;
        let b = matrix(n, n);
        let mut a = vec![C64::ZERO; n * n];
        for r in 0..n {
            for c in 0..n {
                a[r * n + c] = (0..n).map(|j| b[r * n + j] * b[c * n + j].conj()).sum();
            }
        }
        let mut upper = a.clone();
        for r in 0..n {
            for c in 0..r {
                upper[r * n + c] = C64::new(f64::NAN, 0.0);
            }
        }
        let e = hermitian_eig(&upper, n);
        assert!(e.values.windows(2).all(|w| w[0] >= w[1]));
        for r in 0..n {
            for c in 0..n {
                let x: C64 = (0..n)
                    .map(|j| e.vectors[r * n + j] * e.values[j] * e.vectors[c * n + j].conj())
                    .sum();
                assert!((x - a[r * n + c]).norm() < 1e-9 * a[0].norm());
            }
        }
    }
}
