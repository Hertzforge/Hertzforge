//! Small dense helpers on one complex matrix, row-major, over the shared
//! linear algebra of `hertzforge-sdm` (no second decomposition is written
//! here: the SVD and the regularized inverse are that crate's).

use crate::C64;
use hertzforge_sdm::linalg::{pinv, svd, Regularization};
use hertzforge_sdm::MatrixStack;

const ZERO: C64 = C64::new(0.0, 0.0);

/// `y = A x` with `A` `rows x cols`, row-major.
pub fn mat_vec(a: &[C64], rows: usize, cols: usize, x: &[C64], y: &mut [C64]) {
    for (i, yi) in y.iter_mut().enumerate().take(rows) {
        let row = &a[i * cols..(i + 1) * cols];
        *yi = row.iter().zip(x).fold(ZERO, |s, (aij, xj)| s + aij * xj);
    }
}

/// The Euclidean norm of a complex vector.
pub fn norm(x: &[C64]) -> f64 {
    x.iter().map(|v| v.norm_sqr()).sum::<f64>().sqrt()
}

/// The regularized pseudo-inverse of one `rows x cols` matrix,
/// `Z = V diag(s / (s^2 + (eps s_max)^2)) U^H`: the inverse of a square matrix,
/// least squares for more rows than columns, least norm for fewer. With
/// `eps = 0` it is the Moore-Penrose inverse. Returns `cols x rows`, row-major,
/// and the condition number `s_max / s_min`.
pub fn reg_pinv(a: &[C64], rows: usize, cols: usize, eps: f64) -> Result<(Vec<C64>, f64), String> {
    let stack = MatrixStack::from_vec(1, rows, cols, a.to_vec()).map_err(|e| e.to_string())?;
    let reg = if eps > 0.0 {
        Regularization::Tikhonov { lambda_rel: eps }
    } else {
        Regularization::None
    };
    let (z, rep) = pinv(&stack, reg).map_err(|e| e.to_string())?;
    Ok((z.into_data(), rep.cond_overall[0]))
}

/// The singular values of one matrix, descending.
pub fn singular_values(a: &[C64], rows: usize, cols: usize) -> Result<Vec<f64>, String> {
    let stack = MatrixStack::from_vec(1, rows, cols, a.to_vec()).map_err(|e| e.to_string())?;
    Ok(svd(&stack).map_err(|e| e.to_string())?.s)
}

/// The part of `x` (length `cols`) in the row space of `a` (`rows x cols`,
/// `rows < cols`): `V V^H x` with `V` the right singular vectors of the
/// non-zero singular values. What is removed, `x - V V^H x`, is the drive the
/// rows cannot see.
pub fn row_space_part(a: &[C64], rows: usize, cols: usize, x: &[C64]) -> Result<Vec<C64>, String> {
    let stack = MatrixStack::from_vec(1, rows, cols, a.to_vec()).map_err(|e| e.to_string())?;
    let d = svd(&stack).map_err(|e| e.to_string())?;
    let k = d.k();
    let smax = d.s.first().copied().unwrap_or(0.0);
    let floor = rows.max(cols) as f64 * f64::EPSILON * smax;
    let v = d.v.line(0); // cols x k
    let mut out = vec![ZERO; cols];
    for j in 0..k {
        if d.s[j] <= floor {
            continue;
        }
        let c = (0..cols).fold(ZERO, |s, i| s + v[i * k + j].conj() * x[i]);
        for (i, o) in out.iter_mut().enumerate() {
            *o += v[i * k + j] * c;
        }
    }
    Ok(out)
}

/// Solve the real system `A x = b` (`n x n`, row-major) by Gaussian
/// elimination with partial pivoting. `None` when a pivot is exactly zero.
pub fn solve_real(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut m = a.to_vec();
    let mut x = b.to_vec();
    for col in 0..n {
        let p = (col..n).max_by(|&i, &j| m[i * n + col].abs().total_cmp(&m[j * n + col].abs()))?;
        if m[p * n + col] == 0.0 {
            return None;
        }
        if p != col {
            for c in 0..n {
                m.swap(col * n + c, p * n + c);
            }
            x.swap(col, p);
        }
        for r in col + 1..n {
            let f = m[r * n + col] / m[col * n + col];
            if f != 0.0 {
                for c in col..n {
                    m[r * n + c] -= f * m[col * n + c];
                }
                x[r] -= f * x[col];
            }
        }
    }
    for col in (0..n).rev() {
        let s = (col + 1..n).fold(x[col], |s, c| s - m[col * n + c] * x[c]);
        x[col] = s / m[col * n + col];
    }
    Some(x)
}
