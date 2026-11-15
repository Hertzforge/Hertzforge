//! Small dense solvers for the modal fit, the tracking update and the
//! uncertainty-weighted inverse: real least squares by Householder QR, an
//! orthonormal basis by twice-repeated Gram-Schmidt, a real symmetric
//! positive-definite solve, a complex Hermitian positive-definite solve and
//! the spectral norm of a small complex matrix. Nothing here allocates more
//! than its inputs' size, and nothing uses a clock, thread or random number.

use crate::C64;

/// Triangularizes the row-major `m x n` matrix `a` in place by Householder
/// reflections applied from the left (`Q^T a = R`). The upper triangle of the
/// first `min(m, n)` rows holds `R`; the rest is left as zero.
pub fn householder_triangularize(a: &mut [f64], m: usize, n: usize) {
    let kmax = m.min(n);
    let mut v = vec![0.0; m];
    for k in 0..kmax {
        let norm: f64 = (k..m)
            .map(|i| a[i * n + k] * a[i * n + k])
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 {
            continue;
        }
        let alpha = if a[k * n + k] > 0.0 { -norm } else { norm };
        for i in k..m {
            v[i] = a[i * n + k];
        }
        v[k] -= alpha;
        let vnorm2: f64 = (k..m).map(|i| v[i] * v[i]).sum();
        if vnorm2 == 0.0 {
            continue;
        }
        for j in k..n {
            let dot: f64 = (k..m).map(|i| v[i] * a[i * n + j]).sum();
            let f = 2.0 * dot / vnorm2;
            for i in k..m {
                a[i * n + j] -= f * v[i];
            }
        }
        for i in (k + 1)..m {
            a[i * n + k] = 0.0;
        }
    }
}

/// Solves the upper-triangular `n x n` system held in the first `n` rows and
/// columns of the row-major matrix `r` (row stride `stride`) for the right side
/// `b`. A pivot smaller than `tol` times the largest pivot gives a zero
/// component (a least-norm choice for a dependent column).
pub fn back_substitute(r: &[f64], stride: usize, n: usize, b: &[f64], tol: f64) -> Vec<f64> {
    let big = (0..n).fold(0.0f64, |a, i| a.max(r[i * stride + i].abs()));
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let p = r[i * stride + i];
        if p.abs() <= tol * big || p == 0.0 {
            x[i] = 0.0;
            continue;
        }
        let mut s = b[i];
        for j in (i + 1)..n {
            s -= r[i * stride + j] * x[j];
        }
        x[i] = s / p;
    }
    x
}

/// Least squares `min ||a x - b||` for a row-major `m x n` matrix (`m >= n`),
/// with every column scaled to unit norm first. Returns `x` and the residual
/// norm.
pub fn lstsq(a: &[f64], m: usize, n: usize, b: &[f64]) -> Result<(Vec<f64>, f64), String> {
    if a.len() != m * n || b.len() != m {
        return Err("least squares: the matrix and right side do not agree".into());
    }
    if m < n {
        return Err("least squares: fewer rows than columns".into());
    }
    let mut scale = vec![0.0; n];
    for (j, s) in scale.iter_mut().enumerate() {
        *s = (0..m)
            .map(|i| a[i * n + j] * a[i * n + j])
            .sum::<f64>()
            .sqrt();
        if *s == 0.0 {
            *s = 1.0;
        }
    }
    let w = n + 1;
    let mut aug = vec![0.0; m * w];
    for i in 0..m {
        for j in 0..n {
            aug[i * w + j] = a[i * n + j] / scale[j];
        }
        aug[i * w + n] = b[i];
    }
    householder_triangularize(&mut aug, m, w);
    let qtb: Vec<f64> = (0..n).map(|i| aug[i * w + n]).collect();
    let resid = if m > n { aug[n * w + n].abs() } else { 0.0 };
    let xs = back_substitute(&aug, w, n, &qtb, 1e-13);
    Ok((xs.iter().zip(&scale).map(|(x, s)| x / s).collect(), resid))
}

/// An orthonormal basis of the columns of the row-major `m x n` matrix `a`,
/// by modified Gram-Schmidt run twice, with the triangular factor: `a = q r`.
/// `q` is `m x n` row-major, `r` is `n x n` row-major upper triangular. A
/// dependent column gets a zero column in `q` and a zero pivot in `r`.
pub fn orthonormal_basis(a: &[f64], m: usize, n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut q = a.to_vec();
    let mut r = vec![0.0; n * n];
    let col_norm = |q: &[f64], j: usize| (0..m).map(|i| q[i * n + j].powi(2)).sum::<f64>().sqrt();
    let norms0: Vec<f64> = (0..n).map(|j| col_norm(a, j)).collect();
    for j in 0..n {
        for _pass in 0..2 {
            for k in 0..j {
                let dot: f64 = (0..m).map(|i| q[i * n + k] * q[i * n + j]).sum();
                r[k * n + j] += dot;
                for i in 0..m {
                    q[i * n + j] -= dot * q[i * n + k];
                }
            }
        }
        let nrm = col_norm(&q, j);
        if nrm <= 1e-12 * norms0[j].max(f64::MIN_POSITIVE) || nrm == 0.0 {
            for i in 0..m {
                q[i * n + j] = 0.0;
            }
            r[j * n + j] = 0.0;
        } else {
            r[j * n + j] = nrm;
            for i in 0..m {
                q[i * n + j] /= nrm;
            }
        }
    }
    (q, r)
}

/// Solves the real symmetric positive-definite `n x n` system `a x = b` by
/// Cholesky, adding `ridge` times the mean diagonal to the diagonal.
pub fn spd_solve(a: &[f64], n: usize, b: &[f64], ridge: f64) -> Result<Vec<f64>, String> {
    let mean_diag = (0..n).map(|i| a[i * n + i]).sum::<f64>() / n.max(1) as f64;
    let add = ridge * mean_diag.abs().max(f64::MIN_POSITIVE);
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i * n + j] + if i == j { add } else { 0.0 };
            for k in 0..j {
                s -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if s <= 0.0 || !s.is_finite() {
                    return Err("the system is not positive definite".into());
                }
                l[i * n + i] = s.sqrt();
            } else {
                l[i * n + j] = s / l[j * n + j];
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= l[i * n + k] * y[k];
        }
        y[i] = s / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        for k in (i + 1)..n {
            s -= l[k * n + i] * x[k];
        }
        x[i] = s / l[i * n + i];
    }
    Ok(x)
}

/// Solves `a X = b` for a complex Hermitian positive-definite `n x n` matrix
/// `a` (row-major) and an `n x k` right side `b` (row-major), by Cholesky.
pub fn hpd_solve(a: &[C64], n: usize, b: &[C64], k: usize) -> Result<Vec<C64>, String> {
    let zero = C64::new(0.0, 0.0);
    let mut l = vec![zero; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i * n + j];
            for p in 0..j {
                s -= l[i * n + p] * l[j * n + p].conj();
            }
            if i == j {
                if s.re <= 0.0 || !s.re.is_finite() {
                    return Err("the system is not positive definite".into());
                }
                l[i * n + i] = C64::new(s.re.sqrt(), 0.0);
            } else {
                l[i * n + j] = s / l[j * n + j].re;
            }
        }
    }
    let mut x = b.to_vec();
    for c in 0..k {
        for i in 0..n {
            let mut s = x[i * k + c];
            for p in 0..i {
                s -= l[i * n + p] * x[p * k + c];
            }
            x[i * k + c] = s / l[i * n + i].re;
        }
        for i in (0..n).rev() {
            let mut s = x[i * k + c];
            for p in (i + 1)..n {
                s -= l[p * n + i].conj() * x[p * k + c];
            }
            x[i * k + c] = s / l[i * n + i].re;
        }
    }
    Ok(x)
}

/// The largest singular value of a complex `r x c` matrix (row-major), by
/// power iteration on `m^H m` from a fixed start (deterministic).
pub fn spectral_norm(m: &[C64], r: usize, c: usize) -> f64 {
    if r == 0 || c == 0 {
        return 0.0;
    }
    let mut v: Vec<C64> = (0..c)
        .map(|j| C64::new(1.0 + 0.1 * j as f64, 0.05 * j as f64))
        .collect();
    let mut sigma = 0.0;
    let mut u = vec![C64::new(0.0, 0.0); r];
    for _ in 0..60 {
        let nv: f64 = v.iter().map(|x| x.norm_sqr()).sum::<f64>().sqrt();
        if nv == 0.0 {
            return 0.0;
        }
        for x in v.iter_mut() {
            *x /= nv;
        }
        for (i, ui) in u.iter_mut().enumerate() {
            *ui = (0..c).map(|j| m[i * c + j] * v[j]).sum();
        }
        let nu: f64 = u.iter().map(|x| x.norm_sqr()).sum::<f64>().sqrt();
        let next: Vec<C64> = (0..c)
            .map(|j| (0..r).map(|i| m[i * c + j].conj() * u[i]).sum())
            .collect();
        let prev = sigma;
        sigma = nu;
        v = next;
        if (sigma - prev).abs() <= 1e-12 * sigma {
            break;
        }
    }
    sigma
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_lstsq_recovers_an_exact_solution() {
        // 5 x 3 with a known solution.
        let a = [
            1.0, 2.0, 0.5, //
            0.0, 1.0, -1.0, //
            3.0, 0.0, 2.0, //
            1.0, 1.0, 1.0, //
            -2.0, 0.5, 0.0,
        ];
        let x = [0.3, -1.2, 2.5];
        let b: Vec<f64> = (0..5)
            .map(|i| (0..3).map(|j| a[i * 3 + j] * x[j]).sum())
            .collect();
        let (got, res) = lstsq(&a, 5, 3, &b).unwrap();
        for (g, w) in got.iter().zip(x) {
            assert!((g - w).abs() < 1e-12, "{got:?}");
        }
        assert!(res < 1e-12);
        let (q, r) = orthonormal_basis(&a, 5, 3);
        for i in 0..5 {
            for j in 0..3 {
                let v: f64 = (0..3).map(|k| q[i * 3 + k] * r[k * 3 + j]).sum();
                assert!((v - a[i * 3 + j]).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn mimo_shock_hermitian_solve_and_spectral_norm() {
        let a = [
            C64::new(4.0, 0.0),
            C64::new(1.0, 1.0),
            C64::new(1.0, -1.0),
            C64::new(3.0, 0.0),
        ];
        let x = [C64::new(1.0, 2.0), C64::new(-0.5, 0.25)];
        let b = [a[0] * x[0] + a[1] * x[1], a[2] * x[0] + a[3] * x[1]];
        let got = hpd_solve(&a, 2, &b, 1).unwrap();
        for (g, w) in got.iter().zip(x) {
            assert!((g - w).norm() < 1e-12);
        }
        // Singular values of diag(3, 2) rotated stay 3 and 2.
        let m = [
            C64::new(0.0, 3.0),
            C64::new(0.0, 0.0),
            C64::new(0.0, 0.0),
            C64::new(2.0, 0.0),
        ];
        assert!((spectral_norm(&m, 2, 2) - 3.0).abs() < 1e-9);
        let s = spd_solve(&[2.0, 1.0, 1.0, 3.0], 2, &[3.0, 4.0], 0.0).unwrap();
        assert!((s[0] - 1.0).abs() < 1e-12 && (s[1] - 1.0).abs() < 1e-12);
    }
}
