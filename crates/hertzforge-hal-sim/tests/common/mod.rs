//! Small numerical helpers shared by this crate's plant tests: complex matrix inverse,
//! the condition number of a complex matrix, and a seeded generator. Test code only;
//! the workspace has no linear-algebra crate yet and these matrices are small.

#![allow(dead_code)]
// Dense linear algebra written with explicit indices reads as the textbook does.
#![allow(clippy::needless_range_loop)]

use hertzforge_hal_sim::C64;

pub fn mag(c: C64) -> f64 {
    (c.re * c.re + c.im * c.im).sqrt()
}

pub fn arg(c: C64) -> f64 {
    c.im.atan2(c.re)
}

pub fn cdiv(a: C64, b: C64) -> C64 {
    let d = b.re * b.re + b.im * b.im;
    let p = a * b.conj();
    C64::new(p.re / d, p.im / d)
}

/// Inverse of an `n x n` complex matrix (row-major) by Gauss-Jordan elimination with
/// partial pivoting. `None` when a pivot vanishes.
pub fn inverse(a: &[C64], n: usize) -> Option<Vec<C64>> {
    let mut m = a.to_vec();
    let mut inv = vec![C64::new(0.0, 0.0); n * n];
    for i in 0..n {
        inv[i * n + i] = C64::new(1.0, 0.0);
    }
    for col in 0..n {
        let piv = (col..n).max_by(|&x, &y| {
            mag(m[x * n + col])
                .partial_cmp(&mag(m[y * n + col]))
                .unwrap()
        })?;
        if mag(m[piv * n + col]) == 0.0 {
            return None;
        }
        for k in 0..n {
            m.swap(col * n + k, piv * n + k);
            inv.swap(col * n + k, piv * n + k);
        }
        let p = m[col * n + col];
        for k in 0..n {
            m[col * n + k] = cdiv(m[col * n + k], p);
            inv[col * n + k] = cdiv(inv[col * n + k], p);
        }
        for r in 0..n {
            if r != col {
                let f = m[r * n + col];
                if f != C64::new(0.0, 0.0) {
                    for k in 0..n {
                        let a = m[col * n + k];
                        let b = inv[col * n + k];
                        m[r * n + k] -= f * a;
                        inv[r * n + k] -= f * b;
                    }
                }
            }
        }
    }
    Some(inv)
}

/// The 2-norm condition number of a `rows x cols` complex matrix (row-major, rows >=
/// cols): `sqrt(lambda_max / lambda_min)` of `H^H H`. The Hermitian matrix is embedded
/// as the real symmetric `[[Re, -Im], [Im, Re]]` (same eigenvalues, each twice),
/// reduced to tridiagonal form by Householder reflections, and its extreme eigenvalues
/// are found by Sturm-sequence bisection. Backward stable, so `lambda_min` is good to
/// about `1e-16 * lambda_max`, ample for condition numbers up to about 1e6.
pub fn condition_number(h: &[C64], rows: usize, cols: usize) -> f64 {
    let n = cols;
    let mut g = vec![C64::new(0.0, 0.0); n * n];
    for a in 0..n {
        for b in a..n {
            let mut s = C64::new(0.0, 0.0);
            for r in 0..rows {
                s += h[r * cols + a].conj() * h[r * cols + b];
            }
            g[a * n + b] = s;
            g[b * n + a] = s.conj();
        }
    }
    let m = 2 * n;
    let mut a = vec![0.0f64; m * m];
    for r in 0..n {
        for c in 0..n {
            let v = g[r * n + c];
            a[r * m + c] = v.re;
            a[r * m + (c + n)] = -v.im;
            a[(r + n) * m + c] = v.im;
            a[(r + n) * m + (c + n)] = v.re;
        }
    }
    let (d, e) = tridiagonalize(&mut a, m);
    let lo = smallest_eigenvalue(&d, &e);
    let hi = largest_eigenvalue(&d, &e);
    if lo <= 0.0 {
        return f64::INFINITY;
    }
    (hi / lo).sqrt()
}

/// Householder reduction of a real symmetric `m x m` matrix (row-major, overwritten) to
/// tridiagonal form: returns the diagonal and the sub-diagonal.
fn tridiagonalize(a: &mut [f64], m: usize) -> (Vec<f64>, Vec<f64>) {
    let mut v = vec![0.0f64; m];
    let mut p = vec![0.0f64; m];
    for k in 0..m.saturating_sub(2) {
        let lo = k + 1;
        let norm: f64 = (lo..m)
            .map(|i| a[i * m + k] * a[i * m + k])
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 {
            continue;
        }
        let x0 = a[lo * m + k];
        let alpha = if x0 > 0.0 { -norm } else { norm };
        for i in lo..m {
            v[i] = a[i * m + k];
        }
        v[lo] -= alpha;
        let vn: f64 = (lo..m).map(|i| v[i] * v[i]).sum::<f64>().sqrt();
        if vn == 0.0 {
            continue;
        }
        for i in lo..m {
            v[i] /= vn;
        }
        // p = A v on the trailing block; K = v^T p; q = p - K v; A -= 2 (v q^T + q v^T).
        for i in lo..m {
            p[i] = (lo..m).map(|j| a[i * m + j] * v[j]).sum();
        }
        let kk: f64 = (lo..m).map(|i| v[i] * p[i]).sum();
        for i in lo..m {
            p[i] -= kk * v[i];
        }
        for i in lo..m {
            for j in lo..m {
                a[i * m + j] -= 2.0 * (v[i] * p[j] + p[i] * v[j]);
            }
        }
        a[lo * m + k] = alpha;
        a[k * m + lo] = alpha;
        for i in lo + 1..m {
            a[i * m + k] = 0.0;
            a[k * m + i] = 0.0;
        }
    }
    let d = (0..m).map(|i| a[i * m + i]).collect();
    let e = (0..m.saturating_sub(1))
        .map(|i| a[(i + 1) * m + i])
        .collect();
    (d, e)
}

/// Eigenvalues of the tridiagonal `(d, e)` below `x` (Sturm count).
fn count_below(d: &[f64], e: &[f64], x: f64) -> usize {
    let mut count = 0;
    let mut q = d[0] - x;
    if q < 0.0 {
        count += 1;
    }
    for i in 1..d.len() {
        let qq = if q == 0.0 { 1e-300 } else { q };
        q = d[i] - x - e[i - 1] * e[i - 1] / qq;
        if q < 0.0 {
            count += 1;
        }
    }
    count
}

fn gershgorin(d: &[f64], e: &[f64]) -> (f64, f64) {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for i in 0..d.len() {
        let r =
            if i > 0 { e[i - 1].abs() } else { 0.0 } + if i < e.len() { e[i].abs() } else { 0.0 };
        lo = lo.min(d[i] - r);
        hi = hi.max(d[i] + r);
    }
    (lo, hi)
}

/// The `k`-th smallest eigenvalue (0-based) by bisection.
fn kth_eigenvalue(d: &[f64], e: &[f64], k: usize) -> f64 {
    let (mut lo, mut hi) = gershgorin(d, e);
    let span = (hi - lo).abs().max(f64::MIN_POSITIVE);
    lo -= 1e-12 * span;
    hi += 1e-12 * span;
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if mid == lo || mid == hi {
            break;
        }
        if count_below(d, e, mid) > k {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    0.5 * (lo + hi)
}

fn smallest_eigenvalue(d: &[f64], e: &[f64]) -> f64 {
    kth_eigenvalue(d, e, 0)
}

fn largest_eigenvalue(d: &[f64], e: &[f64]) -> f64 {
    kth_eigenvalue(d, e, d.len() - 1)
}

/// A seeded xorshift64* generator for test signals (independent of the plant's).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}
