//! Convergence checks a law check evaluates before a pulse (the conditions of
//! the nonlinear-phase brief, section 8(b), per line).
//!
//! - [`drive_side`]: for the law's inverse `G` and a plant `H` (the true or a
//!   perturbed one), the eigenvalues `lambda_i` of `G H` (`n_d x n_d`) per line.
//!   The waveform correction `D+ = D + g G (R - H D)` contracts on the drive
//!   side iff `abs(1 - g lambda_i) < 1` for every `i`, that is
//!   `g < 2 Re(lambda_i) / abs(lambda_i)^2`. The check reports the safe gain per
//!   line, the spectral radius at a given gain, and the worst line. It never
//!   looks at the output side `I - g H G`, which has `n_c - n_d` eigenvalues
//!   fixed at 1 when there are more controls than drives.
//! - [`reachable_fraction`]: the share of each control's target energy in the
//!   range of `H`: the floor no law can beat when there are more controls than
//!   drives.

use crate::{MatrixStack, C64};
use hertzforge_sdm::linalg::{pinv, Regularization};

/// The eigenvalues of a general complex `n x n` matrix (row-major), by
/// reduction to Hessenberg form and shifted QR. Meant for the small matrices
/// of a law check (a few drives).
pub fn eigenvalues(a: &[C64], n: usize) -> Result<Vec<C64>, String> {
    if a.len() != n * n {
        return Err("the matrix must be n x n".into());
    }
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut h: Vec<C64> = a.to_vec();
    let at = |i: usize, j: usize| i * n + j;
    let zero = C64::new(0.0, 0.0);
    // Householder reduction to upper Hessenberg form (a similarity).
    for k in 0..n.saturating_sub(2) {
        let alpha: f64 = ((k + 1)..n)
            .map(|i| h[at(i, k)].norm_sqr())
            .sum::<f64>()
            .sqrt();
        if alpha == 0.0 {
            continue;
        }
        let x0 = h[at(k + 1, k)];
        let phase = if x0.norm() > 0.0 {
            x0 / x0.norm()
        } else {
            C64::new(1.0, 0.0)
        };
        let mut v: Vec<C64> = vec![zero; n];
        for i in (k + 1)..n {
            v[i] = h[at(i, k)];
        }
        v[k + 1] += phase * alpha;
        let vn: f64 = v.iter().map(|x| x.norm_sqr()).sum();
        if vn == 0.0 {
            continue;
        }
        // H <- (I - 2 v v^H / vn) H (I - 2 v v^H / vn)
        for j in 0..n {
            let mut s = zero;
            for i in (k + 1)..n {
                s += v[i].conj() * h[at(i, j)];
            }
            let s = s * (2.0 / vn);
            for i in (k + 1)..n {
                h[at(i, j)] -= v[i] * s;
            }
        }
        for i in 0..n {
            let mut s = zero;
            for j in (k + 1)..n {
                s += h[at(i, j)] * v[j];
            }
            let s = s * (2.0 / vn);
            for j in (k + 1)..n {
                h[at(i, j)] -= s * v[j].conj();
            }
        }
    }
    let mut eig = vec![zero; n];
    let mut hi = n - 1;
    let mut iter = 0usize;
    let mut total = 0usize;
    loop {
        if hi == 0 {
            eig[0] = h[at(0, 0)];
            break;
        }
        // Find the start of the active unreduced block.
        let mut lo = hi;
        while lo > 0 {
            let s = h[at(lo - 1, lo - 1)].norm() + h[at(lo, lo)].norm();
            let s = if s == 0.0 { 1.0 } else { s };
            if h[at(lo, lo - 1)].norm() <= f64::EPSILON * s {
                h[at(lo, lo - 1)] = zero;
                break;
            }
            lo -= 1;
        }
        if lo == hi {
            eig[hi] = h[at(hi, hi)];
            hi -= 1;
            iter = 0;
            continue;
        }
        iter += 1;
        total += 1;
        if total > 100 * n.max(4) {
            return Err("the eigenvalue iteration did not converge".into());
        }
        // Wilkinson shift from the trailing 2 x 2, with an exceptional shift
        // every tenth iteration on one block.
        let (a11, a12, a21, a22) = (
            h[at(hi - 1, hi - 1)],
            h[at(hi - 1, hi)],
            h[at(hi, hi - 1)],
            h[at(hi, hi)],
        );
        let mu = if iter.is_multiple_of(10) {
            a22 + C64::new(h[at(hi, hi - 1)].norm(), 0.0)
        } else {
            let tr = a11 + a22;
            let det = a11 * a22 - a12 * a21;
            let disc = (tr * tr * 0.25 - det).sqrt();
            let l1 = tr * 0.5 + disc;
            let l2 = tr * 0.5 - disc;
            if (l1 - a22).norm() < (l2 - a22).norm() {
                l1
            } else {
                l2
            }
        };
        for i in lo..=hi {
            h[at(i, i)] -= mu;
        }
        // QR by Givens rotations on rows lo..=hi, then RQ.
        let mut rots: Vec<(f64, C64)> = Vec::with_capacity(hi - lo);
        for k in lo..hi {
            let x = h[at(k, k)];
            let y = h[at(k + 1, k)];
            let r = (x.norm_sqr() + y.norm_sqr()).sqrt();
            let (c, s) = if r == 0.0 {
                (1.0, zero)
            } else if x.norm() == 0.0 {
                (0.0, C64::new(1.0, 0.0))
            } else {
                (x.norm() / r, (x / x.norm()) * y.conj() / r)
            };
            for j in k..n {
                let p = h[at(k, j)];
                let q = h[at(k + 1, j)];
                h[at(k, j)] = p * c + s * q;
                h[at(k + 1, j)] = -s.conj() * p + q * c;
            }
            rots.push((c, s));
        }
        for (idx, k) in (lo..hi).enumerate() {
            let (c, s) = rots[idx];
            for i in 0..=(k + 1).min(hi) {
                let p = h[at(i, k)];
                let q = h[at(i, k + 1)];
                h[at(i, k)] = p * c + q * s.conj();
                h[at(i, k + 1)] = -p * s + q * c;
            }
        }
        for i in lo..=hi {
            h[at(i, i)] += mu;
        }
    }
    Ok(eig)
}

/// The drive-side check of a law's inverse against one plant.
#[derive(Clone, Debug, PartialEq)]
pub struct DriveSide {
    /// Per line, the largest gain at which every drive-side eigenvalue
    /// contracts: `min_i 2 Re(lambda_i) / abs(lambda_i)^2`, zero when some
    /// `Re(lambda_i) <= 0`. Eigenvalues the inverse does not reach
    /// (`abs(lambda) <= 1e-12`) are skipped. Infinite on lines with none.
    pub safe_gain: Vec<f64>,
    /// Per line, the spectral radius of `I - g G H` at the gain asked for.
    pub radius: Vec<f64>,
    /// The largest radius over the masked lines, and its line.
    pub worst: (f64, usize),
    /// The smallest safe gain over the masked lines, and its line.
    pub least_safe: (f64, usize),
}

/// The drive-side eigenvalues of `G H` per masked line, with the safe gain and
/// the radius at `gain`. `inverse` is `n_d x n_c`, `plant` `n_c x n_d`.
pub fn drive_side(
    inverse: &MatrixStack,
    plant: &MatrixStack,
    mask: &[bool],
    gain: f64,
) -> Result<DriveSide, String> {
    let (lines, n_d, n_c) = inverse.dims();
    if plant.dims() != (lines, n_c, n_d) || mask.len() != lines {
        return Err("the inverse, plant and mask do not share lines and sizes".into());
    }
    let mut out = DriveSide {
        safe_gain: vec![f64::INFINITY; lines],
        radius: vec![0.0; lines],
        worst: (0.0, 0),
        least_safe: (f64::INFINITY, 0),
    };
    let mut m = vec![C64::new(0.0, 0.0); n_d * n_d];
    for (f, _) in mask.iter().enumerate().filter(|(_, &on)| on) {
        let g = inverse.line(f);
        let h = plant.line(f);
        for i in 0..n_d {
            for j in 0..n_d {
                let mut acc = C64::new(0.0, 0.0);
                for c in 0..n_c {
                    acc += g[i * n_c + c] * h[c * n_d + j];
                }
                m[i * n_d + j] = acc;
            }
        }
        let lams = eigenvalues(&m, n_d)?;
        let mut safe = f64::INFINITY;
        let mut rad = 0.0f64;
        for l in &lams {
            rad = rad.max((C64::new(1.0, 0.0) - l * gain).norm());
            if l.norm() <= 1e-12 {
                continue;
            }
            safe = if l.re <= 0.0 {
                0.0
            } else {
                safe.min(2.0 * l.re / l.norm_sqr())
            };
        }
        out.safe_gain[f] = safe;
        out.radius[f] = rad;
        if rad > out.worst.0 {
            out.worst = (rad, f);
        }
        if safe < out.least_safe.0 {
            out.least_safe = (safe, f);
        }
    }
    Ok(out)
}

/// The share of each control's target energy (masked lines) that lies in the
/// range of the plant: `1 - sum_f abs(((I - H H^+) R)_c)^2 / sum_f abs(R_c)^2`
/// with `H^+` the Moore-Penrose pseudo-inverse. One for every control when the
/// plant has full row rank; below one is the floor no law can beat.
pub fn reachable_fraction(
    plant: &MatrixStack,
    mask: &[bool],
    reference: &[Vec<C64>],
) -> Result<Vec<f64>, String> {
    let (lines, n_c, n_d) = plant.dims();
    if mask.len() != lines || reference.len() != n_c || reference.iter().any(|r| r.len() != lines) {
        return Err("the plant, mask and reference do not share lines and sizes".into());
    }
    let (hp, _) = pinv(plant, Regularization::None).map_err(|e| e.to_string())?;
    let mut lost = vec![0.0; n_c];
    let mut total = vec![0.0; n_c];
    let mut d = vec![C64::new(0.0, 0.0); n_d];
    for (f, _) in mask.iter().enumerate().filter(|(_, &on)| on) {
        let g = hp.line(f);
        let h = plant.line(f);
        for (i, di) in d.iter_mut().enumerate() {
            *di = (0..n_c).map(|c| g[i * n_c + c] * reference[c][f]).sum();
        }
        for c in 0..n_c {
            let y: C64 = (0..n_d).map(|i| h[c * n_d + i] * d[i]).sum();
            lost[c] += (reference[c][f] - y).norm_sqr();
            total[c] += reference[c][f].norm_sqr();
        }
    }
    Ok(lost
        .iter()
        .zip(&total)
        .map(|(l, t)| if *t > 0.0 { 1.0 - l / t } else { 1.0 })
        .collect())
}

/// The local convergence of `shape` on one line (`Derivations.md` D6).
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeJacobian {
    /// Masked lines whose Jacobian is not diagonally dominant by rows
    /// (some `J_cc <= sum_{j != c} abs(J_cj)`).
    pub lines_without_dominance: usize,
    /// Masked lines with an eigenvalue of nonpositive real part: no weight
    /// makes the magnitude loop contract there.
    pub lines_unstable: usize,
    /// Masked lines examined.
    pub lines: usize,
    /// The smallest safe weight over the masked lines,
    /// `min 2 Re(lambda) / abs(lambda)^2` (zero where a line is unstable).
    pub least_safe_weight: f64,
}

/// The real Jacobian of `log abs(Y_c)` against `log C_j` per masked line, for
/// the response `Y = H G (C o R)` of the shaped targets `C o R` (given here as
/// `targets`): `J_cj = Re(M_cj X_j / Y_c)` with `M = H G` and `X` the shaped
/// targets. Its rows sum to one. `shape` contracts on a line when every
/// eigenvalue `lambda` of `J` satisfies `abs(1 - w lambda) < 1`. Lines where a
/// control's response is below `1e-9` of the largest are skipped.
pub fn shape_jacobian(
    inverse: &MatrixStack,
    plant: &MatrixStack,
    mask: &[bool],
    targets: &[Vec<C64>],
) -> Result<ShapeJacobian, String> {
    let (lines, n_d, n_c) = inverse.dims();
    if plant.dims() != (lines, n_c, n_d)
        || mask.len() != lines
        || targets.len() != n_c
        || targets.iter().any(|t| t.len() != lines)
    {
        return Err("the inverse, plant, mask and targets do not share lines and sizes".into());
    }
    let mut m = vec![C64::new(0.0, 0.0); n_c * n_c];
    let mut y = vec![C64::new(0.0, 0.0); n_c];
    let mut j = vec![C64::new(0.0, 0.0); n_c * n_c];
    let mut out = ShapeJacobian {
        lines_without_dominance: 0,
        lines_unstable: 0,
        lines: 0,
        least_safe_weight: f64::INFINITY,
    };
    let mut peak = 0.0f64;
    for f in 0..lines {
        for t in targets {
            peak = peak.max(t[f].norm());
        }
    }
    for (f, _) in mask.iter().enumerate().filter(|(_, &on)| on) {
        let g = inverse.line(f);
        let h = plant.line(f);
        for r in 0..n_c {
            for c in 0..n_c {
                m[r * n_c + c] = (0..n_d).map(|i| h[r * n_d + i] * g[i * n_c + c]).sum();
            }
            y[r] = (0..n_c).map(|c| m[r * n_c + c] * targets[c][f]).sum();
        }
        if y.iter().any(|v| v.norm() <= 1e-9 * peak) {
            continue;
        }
        out.lines += 1;
        let mut dominant = true;
        for r in 0..n_c {
            let mut off = 0.0;
            for c in 0..n_c {
                let v = (m[r * n_c + c] * targets[c][f] / y[r]).re;
                j[r * n_c + c] = C64::new(v, 0.0);
                if c != r {
                    off += v.abs();
                }
            }
            if j[r * n_c + r].re <= off {
                dominant = false;
            }
        }
        if !dominant {
            out.lines_without_dominance += 1;
        }
        let mut safe = f64::INFINITY;
        for l in eigenvalues(&j, n_c)? {
            safe = if l.re <= 0.0 {
                0.0
            } else {
                safe.min(2.0 * l.re / l.norm_sqr())
            };
        }
        if safe == 0.0 {
            out.lines_unstable += 1;
        }
        out.least_safe_weight = out.least_safe_weight.min(safe);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(re: f64, im: f64) -> C64 {
        C64::new(re, im)
    }

    fn sorted(mut v: Vec<C64>) -> Vec<C64> {
        v.sort_by(|a, b| a.re.total_cmp(&b.re).then(a.im.total_cmp(&b.im)));
        v
    }

    #[test]
    fn mimo_shock_eigenvalues_of_triangular_and_two_by_two() {
        let a = [c(1.0, 1.0), c(5.0, 0.0), c(0.0, 0.0), c(-2.0, 0.5)];
        let e = sorted(eigenvalues(&a, 2).unwrap());
        assert!((e[0] - c(-2.0, 0.5)).norm() < 1e-12);
        assert!((e[1] - c(1.0, 1.0)).norm() < 1e-12);
        // A rotation: eigenvalues +/- j.
        let r = [c(0.0, 0.0), c(-1.0, 0.0), c(1.0, 0.0), c(0.0, 0.0)];
        let e = sorted(eigenvalues(&r, 2).unwrap());
        assert!((e[0] - c(0.0, -1.0)).norm() < 1e-12, "{e:?}");
        assert!((e[1] - c(0.0, 1.0)).norm() < 1e-12, "{e:?}");
    }

    #[test]
    fn mimo_shock_eigenvalues_satisfy_trace_and_determinant() {
        // A dense 5 x 5 complex matrix from a fixed formula.
        let n = 5;
        let a: Vec<C64> = (0..n * n)
            .map(|k| {
                c(
                    ((k * 7 + 3) % 11) as f64 - 5.0,
                    ((k * 5 + 1) % 13) as f64 - 6.0,
                )
            })
            .collect();
        let e = eigenvalues(&a, n).unwrap();
        let tr: C64 = (0..n).map(|i| a[i * n + i]).sum();
        let sum: C64 = e.iter().sum();
        assert!((tr - sum).norm() < 1e-9 * (1.0 + tr.norm()), "{tr} {sum}");
        // Each eigenvalue makes A - lambda I singular: its smallest singular
        // value is tiny next to the largest.
        for l in &e {
            let mut m = MatrixStack::zeros(1, n, n);
            for i in 0..n {
                for j in 0..n {
                    let v = a[i * n + j] - if i == j { *l } else { c(0.0, 0.0) };
                    m.set(0, i, j, v);
                }
            }
            let s = hertzforge_sdm::linalg::svd(&m).unwrap().s;
            assert!(s[n - 1] < 1e-10 * s[0], "{l}: {s:?}");
        }
    }

    #[test]
    fn mimo_shock_safe_gain_matches_the_scalar_condition() {
        // One drive, one control: G H = exp(j 60 deg). Safe gain 2 cos(60) = 1.
        let mut g = MatrixStack::zeros(1, 1, 1);
        g.set(0, 0, 0, c(1.0, 0.0));
        let mut h = MatrixStack::zeros(1, 1, 1);
        h.set(0, 0, 0, C64::from_polar(1.0, 60f64.to_radians()));
        let d = drive_side(&g, &h, &[true], 1.0).unwrap();
        assert!((d.safe_gain[0] - 1.0).abs() < 1e-12);
        assert!((d.radius[0] - 1.0).abs() < 1e-12);
        let d = drive_side(&g, &h, &[true], 0.5).unwrap();
        assert!(d.radius[0] < 1.0);
    }

    #[test]
    fn mimo_shock_shape_jacobian_is_identity_for_one_control_and_flags_a_flip() {
        // One control: J = 1 whatever the phase of M.
        let mut g = MatrixStack::zeros(1, 1, 1);
        g.set(0, 0, 0, c(1.0, 0.0));
        let mut h = MatrixStack::zeros(1, 1, 1);
        h.set(0, 0, 0, C64::from_polar(2.0, 2.5));
        let j = shape_jacobian(&g, &h, &[true], &[vec![c(1.0, 0.0)]]).unwrap();
        assert_eq!(
            (j.lines, j.lines_unstable, j.lines_without_dominance),
            (1, 0, 0)
        );
        assert!((j.least_safe_weight - 2.0).abs() < 1e-12);
        // Two controls, G = I, H with strong cross terms in opposite phase:
        // M = [[1, 2], [2, 1]], targets (1, 1): Y = (3, 3), J = [[1/3, 2/3],
        // [2/3, 1/3]], eigenvalues 1 and -1/3: unstable, not dominant.
        let mut g = MatrixStack::zeros(1, 2, 2);
        g.set(0, 0, 0, c(1.0, 0.0));
        g.set(0, 1, 1, c(1.0, 0.0));
        let mut h = MatrixStack::zeros(1, 2, 2);
        h.set(0, 0, 0, c(1.0, 0.0));
        h.set(0, 0, 1, c(2.0, 0.0));
        h.set(0, 1, 0, c(2.0, 0.0));
        h.set(0, 1, 1, c(1.0, 0.0));
        let t = vec![vec![c(1.0, 0.0)], vec![c(1.0, 0.0)]];
        let j = shape_jacobian(&g, &h, &[true], &t).unwrap();
        assert_eq!((j.lines_unstable, j.lines_without_dominance), (1, 1));
        assert_eq!(j.least_safe_weight, 0.0);
    }

    #[test]
    fn mimo_shock_reachable_fraction_of_a_tall_plant() {
        // Two controls, one drive with path (1, 0): the second control's
        // target is unreachable, the first fully reachable.
        let mut h = MatrixStack::zeros(1, 2, 1);
        h.set(0, 0, 0, c(1.0, 0.0));
        let r = vec![vec![c(1.0, 0.0)], vec![c(2.0, 0.0)]];
        let f = reachable_fraction(&h, &[true], &r).unwrap();
        assert!((f[0] - 1.0).abs() < 1e-12);
        assert!(f[1].abs() < 1e-12);
    }
}
