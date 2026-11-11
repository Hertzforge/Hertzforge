//! The per-band fit over PSD drives and the real helpers it needs.

use crate::linalg::hermitian_eig;
use hertzforge_sdm::C64;

/// Fits the band response `Re(sum C_p * P)` to the targets `t` over PSD drives
/// `P`: minimizes `sum_p ((r_p - t_p) / t_p)^2` over the rows with `t_p > 0`.
///
/// `P` is factored as `L L^H`, so it is PSD by construction and its trace is
/// `||L||_F^2`. Damped minimum-norm Gauss-Newton on `L`, warm-started from `p0`.
/// `a` is `rows` by `n * n`, one vectorized `C_p` per row.
pub(crate) fn psd_fit(
    a: &[C64],
    rows: usize,
    n: usize,
    t: &[f64],
    p0: &[C64],
) -> Result<Vec<C64>, String> {
    const MAX_ITER: usize = 30;
    const TOL_DB: f64 = 1e-3;
    let n2 = n * n;
    let mut c = Vec::new();
    for p in (0..rows).filter(|&p| t[p] > 0.0) {
        c.extend(a[p * n2..(p + 1) * n2].iter().map(|x| x.scale(1.0 / t[p])));
    }
    let n_rows = c.len() / n2;

    // Every step is L <- (I + M) L, so a zero column of L stays zero: L is kept n by
    // cols, with only the columns of p0's positive eigenvalues. An eigenvalue at
    // rounding level is taken as zero, which moves P by rounding alone.
    let eig = hermitian_eig(&hermitized(p0, n), n);
    let top = eig.values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let floor = 1e-14 * top.max(0.0);
    let mut kept: Vec<(usize, f64)> = eig
        .values
        .iter()
        .enumerate()
        .filter(|x| *x.1 > floor)
        .map(|(r, &w)| (r, w.sqrt()))
        .collect();
    if kept.is_empty() {
        let size = top.max(1e-30).sqrt();
        kept = (0..n).map(|r| (r, size)).collect();
    }
    let cols = kept.len();
    let mut l = vec![C64::ZERO; n * cols];
    for i in 0..n {
        for (to, &(r, size)) in kept.iter().enumerate() {
            l[i * cols + to] = eig.vectors[i * n + r].scale(size);
        }
    }

    let mut r = response(&c, n_rows, n, cols, &l);
    let mut mu: Option<f64> = None;
    let span = n * cols;
    let mut g = vec![0.0; n_rows * 2 * span];
    let mut jj = vec![0.0; n_rows * n_rows];
    for _ in 0..MAX_ITER {
        let res: Vec<f64> = r.iter().map(|x| 1.0 - x).collect();
        // dr_p = 2 Re sum(dL * G_p), with G_p = C_p conj(L) kept as its reals then its
        // imaginaries, so a row of the Jacobian dotted with another is 4 times these dot.
        gradients(&c, n, cols, &l, &mut g);
        for p in 0..n_rows {
            let gp = &g[p * 2 * span..(p + 1) * 2 * span];
            for q in p..n_rows {
                let v = 4.0 * dot(gp, &g[q * 2 * span..(q + 1) * 2 * span]);
                jj[p * n_rows + q] = v;
                jj[q * n_rows + p] = v;
            }
        }
        let scale = (0..n_rows).map(|p| jj[p * n_rows + p]).sum::<f64>() / n_rows as f64;
        let mut damping = mu.unwrap_or(1e-6 * scale);
        let cost = dot(&res, &res);
        let (ln, rn) = loop {
            let mut system = jj.clone();
            for p in 0..n_rows {
                system[p * n_rows + p] += damping;
            }
            let mut y = res.clone();
            if !solve_real(&mut system, &mut y, n_rows) {
                return Err("the PSD fit's normal equations are singular".into());
            }
            let mut ln = l.clone();
            for (p, yp) in y.iter().enumerate() {
                let gp = &g[p * 2 * span..(p + 1) * 2 * span];
                let (g_re, g_im) = gp.split_at(span);
                for ((x, re), im) in ln.iter_mut().zip(g_re).zip(g_im) {
                    x.re += 2.0 * yp * re;
                    x.im -= 2.0 * yp * im;
                }
            }
            let rn = response(&c, n_rows, n, cols, &ln);
            let cost_n: f64 = rn.iter().map(|x| (1.0 - x) * (1.0 - x)).sum();
            if cost_n < cost {
                damping = (damping / 3.0).max(1e-12 * scale);
                break (ln, rn);
            }
            damping *= 4.0;
            // No descent left: converged or stuck. A NaN stops here too.
            let ceiling = 1e6 * scale;
            if damping > ceiling || ceiling.is_nan() || damping.is_nan() {
                return Ok(outer(&l, n, cols));
            }
        };
        mu = Some(damping);
        let moved = rn
            .iter()
            .zip(&r)
            .map(|(a, b)| (10.0 * (a.max(1e-300) / b.max(1e-300)).log10()).abs())
            .fold(f64::NEG_INFINITY, f64::max);
        l = ln;
        r = rn;
        if moved < TOL_DB {
            break;
        }
    }
    Ok(outer(&l, n, cols))
}

/// `0.5 (a + a^H)` of an `n` by `n` matrix.
pub(crate) fn hermitized(a: &[C64], n: usize) -> Vec<C64> {
    let mut out = vec![C64::ZERO; n * n];
    for i in 0..n {
        for j in 0..n {
            out[i * n + j] = (a[i * n + j] + a[j * n + i].conj()).scale(0.5);
        }
    }
    out
}

/// `l l^H` of an `n` by `cols` matrix.
fn outer(l: &[C64], n: usize, cols: usize) -> Vec<C64> {
    let mut p = vec![C64::ZERO; n * n];
    for i in 0..n {
        let row = &l[i * cols..(i + 1) * cols];
        for j in 0..n {
            let other = &l[j * cols..(j + 1) * cols];
            let mut sum = C64::ZERO;
            for (x, y) in row.iter().zip(other) {
                sum.re += x.re * y.re + x.im * y.im;
                sum.im += x.im * y.re - x.re * y.im;
            }
            p[i * n + j] = sum;
        }
    }
    p
}

/// The real part of `sum_ij C_p[i, j] (L L^H)[i, j]` for every row.
fn response(c: &[C64], n_rows: usize, n: usize, cols: usize, l: &[C64]) -> Vec<f64> {
    let p = outer(l, n, cols);
    (0..n_rows)
        .map(|row| real_dot(&c[row * n * n..(row + 1) * n * n], &p))
        .collect()
}

/// `Re(sum a_k b_k)`, without conjugating either.
pub(crate) fn real_dot(a: &[C64], b: &[C64]) -> f64 {
    let mut acc = [0.0; 4];
    for (x, y) in a.chunks_exact(2).zip(b.chunks_exact(2)) {
        acc[0] += x[0].re * y[0].re;
        acc[1] -= x[0].im * y[0].im;
        acc[2] += x[1].re * y[1].re;
        acc[3] -= x[1].im * y[1].im;
    }
    let mut sum = acc[0] + acc[1] + acc[2] + acc[3];
    if a.len() % 2 == 1 {
        let (x, y) = (a[a.len() - 1], b[a.len() - 1]);
        sum += x.re * y.re - x.im * y.im;
    }
    sum
}

/// Each row's `G_p = C_p conj(L)`, `n * cols` reals then `n * cols` imaginaries.
fn gradients(c: &[C64], n: usize, cols: usize, l: &[C64], g: &mut [f64]) {
    let l_re: Vec<f64> = l.iter().map(|x| x.re).collect();
    let l_im: Vec<f64> = l.iter().map(|x| x.im).collect();
    for (cp, gp) in c.chunks_exact(n * n).zip(g.chunks_exact_mut(2 * n * cols)) {
        gp.fill(0.0);
        let (g_re, g_im) = gp.split_at_mut(n * cols);
        for i in 0..n {
            let out_re = &mut g_re[i * cols..(i + 1) * cols];
            let out_im = &mut g_im[i * cols..(i + 1) * cols];
            for j in 0..n {
                let x = cp[i * n + j];
                let lj_re = &l_re[j * cols..(j + 1) * cols];
                let lj_im = &l_im[j * cols..(j + 1) * cols];
                for k in 0..cols {
                    out_re[k] += x.re * lj_re[k] + x.im * lj_im[k];
                    out_im[k] += x.im * lj_re[k] - x.re * lj_im[k];
                }
            }
        }
    }
}

/// Separate running sums, so the compiler can vectorize a floating point sum.
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut acc = [0.0; 4];
    let (a4, b4) = (a.chunks_exact(4), b.chunks_exact(4));
    let tail: f64 = a4
        .remainder()
        .iter()
        .zip(b4.remainder())
        .map(|(x, y)| x * y)
        .sum();
    for (x, y) in a4.zip(b4) {
        for i in 0..4 {
            acc[i] += x[i] * y[i];
        }
    }
    acc[0] + acc[1] + acc[2] + acc[3] + tail
}

/// Solves `a x = b` in place by Gaussian elimination with partial pivoting;
/// `b` becomes `x`. False when a pivot is zero.
pub(crate) fn solve_real(a: &mut [f64], b: &mut [f64], n: usize) -> bool {
    for k in 0..n {
        let mut pivot = k;
        for i in (k + 1)..n {
            if a[i * n + k].abs() > a[pivot * n + k].abs() {
                pivot = i;
            }
        }
        if a[pivot * n + k] == 0.0 {
            return false;
        }
        if pivot != k {
            for j in 0..n {
                a.swap(k * n + j, pivot * n + j);
            }
            b.swap(k, pivot);
        }
        for i in (k + 1)..n {
            let factor = a[i * n + k] / a[k * n + k];
            if factor != 0.0 {
                let (above, below) = a.split_at_mut(i * n);
                let top = &above[k * n + k..(k + 1) * n];
                for (x, y) in below[k..n].iter_mut().zip(top) {
                    *x -= factor * y;
                }
                b[i] -= factor * b[k];
            }
        }
    }
    for k in (0..n).rev() {
        let mut x = b[k];
        for j in (k + 1)..n {
            x -= a[k * n + j] * b[j];
        }
        b[k] = x / a[k * n + k];
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solve_real_needs_the_pivot_swap() {
        let mut a = vec![0.0, 2.0, 3.0, 1.0];
        let mut b = vec![4.0, 5.0];
        assert!(solve_real(&mut a, &mut b, 2));
        assert!((b[0] - 1.0).abs() < 1e-15 && (b[1] - 2.0).abs() < 1e-15);
        let mut singular = vec![1.0, 2.0, 2.0, 4.0];
        assert!(!solve_real(&mut singular, &mut [1.0, 1.0], 2));
    }

    #[test]
    fn the_fit_reaches_targets_a_psd_drive_can_meet() {
        // Two rows that each read one drive's autopower: the answer is diagonal.
        let n = 2;
        let a = vec![
            C64::ONE,
            C64::ZERO,
            C64::ZERO,
            C64::ZERO, //
            C64::ZERO,
            C64::ZERO,
            C64::ZERO,
            C64::ONE,
        ];
        let p0 = vec![C64::new(0.5, 0.0), C64::ZERO, C64::ZERO, C64::new(0.5, 0.0)];
        let p = psd_fit(&a, 2, n, &[2.0, 3.0], &p0).unwrap();
        assert!((p[0].re - 2.0).abs() < 1e-3 && (p[3].re - 3.0).abs() < 1e-3);
        let e = hermitian_eig(&p, n);
        assert!(e.values.iter().all(|&v| v >= -1e-12));
    }
}
