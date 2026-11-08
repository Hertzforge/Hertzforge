//! Tests for the per-line linear algebra. Reference values come from hand
//! computation or from naive triple loops written here, independent of `faer`.

use super::*;
use std::f64::consts::{FRAC_1_SQRT_2, PI};

fn c(re: f64, im: f64) -> C64 {
    C64::new(re, im)
}

/// A small deterministic generator (SplitMix64), so every test is reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn gauss(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-300);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }

    /// Circular complex Gaussian with unit variance.
    fn cplx(&mut self) -> C64 {
        c(self.gauss(), self.gauss()) * FRAC_1_SQRT_2
    }
}

fn random_stack(rng: &mut Rng, lines: usize, rows: usize, cols: usize) -> MatrixStack {
    let data = (0..lines * rows * cols).map(|_| rng.cplx()).collect();
    MatrixStack::from_vec(lines, rows, cols, data).unwrap()
}

/// Row-major `m x k` times `k x n`.
fn mul(a: &[C64], b: &[C64], m: usize, k: usize, n: usize) -> Vec<C64> {
    let mut out = vec![ZERO; m * n];
    for i in 0..m {
        for j in 0..n {
            let mut acc = ZERO;
            for l in 0..k {
                acc += a[i * k + l] * b[l * n + j];
            }
            out[i * n + j] = acc;
        }
    }
    out
}

/// Conjugate transpose of a row-major `m x n`.
fn adj(a: &[C64], m: usize, n: usize) -> Vec<C64> {
    let mut out = vec![ZERO; n * m];
    for i in 0..m {
        for j in 0..n {
            out[j * m + i] = a[i * n + j].conj();
        }
    }
    out
}

fn eye(n: usize) -> Vec<C64> {
    let mut out = vec![ZERO; n * n];
    for i in 0..n {
        out[i * n + i] = ONE;
    }
    out
}

fn diff(a: &[C64], b: &[C64]) -> f64 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).norm())
        .fold(0.0, f64::max)
}

fn maxabs(a: &[C64]) -> f64 {
    a.iter().map(|x| x.norm()).fold(0.0, f64::max)
}

/// One line of a Hermitian stack as a full row-major matrix.
fn full(g: &HermitianStack, line: usize) -> Vec<C64> {
    let n = g.n();
    let mut out = vec![ZERO; n * n];
    for r in 0..n {
        for cc in 0..n {
            out[r * n + cc] = g.get(line, r, cc);
        }
    }
    out
}

fn from_full_lines(n: usize, lines: &[Vec<C64>]) -> HermitianStack {
    let mut data = Vec::with_capacity(lines.len() * n * n);
    for l in lines {
        data.extend_from_slice(l);
    }
    HermitianStack::from_full(&MatrixStack::from_vec(lines.len(), n, n, data).unwrap()).unwrap()
}

/// `A A^H` per line with `A` random `n x rank`: positive semi-definite of that rank.
fn random_psd(rng: &mut Rng, lines: usize, n: usize, rank: usize) -> HermitianStack {
    let per: Vec<Vec<C64>> = (0..lines)
        .map(|_| {
            let a: Vec<C64> = (0..n * rank).map(|_| rng.cplx()).collect();
            mul(&a, &adj(&a, n, rank), n, rank, n)
        })
        .collect();
    from_full_lines(n, &per)
}

/// Random unitary matrices (the eigenvectors of a random definite matrix).
fn random_unitary(rng: &mut Rng, lines: usize, n: usize) -> MatrixStack {
    eigh(&random_psd(rng, lines, n, n)).unwrap().vectors
}

/// `V diag(lambda) V^H` per line with random unitary `V` and the given eigenvalues.
fn with_eigenvalues(rng: &mut Rng, lams: &[Vec<f64>]) -> HermitianStack {
    let n = lams[0].len();
    let v = random_unitary(rng, lams.len(), n);
    let per: Vec<Vec<C64>> = lams
        .iter()
        .enumerate()
        .map(|(line, l)| {
            let vl = v.line(line);
            let mut vd = vl.to_vec();
            for i in 0..n {
                for j in 0..n {
                    vd[i * n + j] *= l[j];
                }
            }
            mul(&vd, &adj(vl, n, n), n, n, n)
        })
        .collect();
    from_full_lines(n, &per)
}

// ---------------------------------------------------------------------------
// Hand-computed cases
// ---------------------------------------------------------------------------

#[test]
fn eigh_returns_the_hand_computed_pair_of_a_2x2() {
    // G = [[2, i], [-i, 2]]: det(G - lambda I) = (2 - lambda)^2 - 1, so lambda = 1, 3.
    // (G - 3I) v = 0 gives v0 = i v1, v = (i, 1) / sqrt(2).
    // (G - I) v = 0 gives v0 = -i v1, v = (-i, 1) / sqrt(2).
    let mut g = HermitianStack::zeros(1, 2);
    g.set(0, 0, 0, c(2.0, 0.0));
    g.set(0, 1, 1, c(2.0, 0.0));
    g.set(0, 0, 1, c(0.0, 1.0));
    let e = eigh(&g).unwrap();
    assert!((e.values[0] - 1.0).abs() < 1e-14, "{:?}", e.values);
    assert!((e.values[1] - 3.0).abs() < 1e-14, "{:?}", e.values);
    let s = FRAC_1_SQRT_2;
    let expect = [[c(0.0, -s), c(s, 0.0)], [c(0.0, s), c(s, 0.0)]];
    for (j, ex) in expect.iter().enumerate() {
        let v = [e.vectors.get(0, 0, j), e.vectors.get(0, 1, j)];
        // The same vector up to a unit phase.
        let ip = ex[0].conj() * v[0] + ex[1].conj() * v[1];
        assert!((ip.norm() - 1.0).abs() < 1e-14, "vector {j}: {v:?}");
        for r in 0..2 {
            let gv = g.get(0, r, 0) * v[0] + g.get(0, r, 1) * v[1];
            assert!((gv - v[r] * e.values[j]).norm() < 1e-14);
        }
    }
}

#[test]
fn a14_the_cross_term_phase_follows_e_x_xh() {
    // One drive, two controls: H = [1, i]^T, so control 1 leads the drive by +90
    // degrees. The drive SDM is the 1 x 1 diagonal G_xx = [2].
    //
    // Y = H X gives Y_0 = X and Y_1 = i X. Under G = E[Y Y^H]:
    //   G_yy[0][1] = E[Y_0 conj(Y_1)] = E[X conj(i X)] = -i E[|X|^2] = -2i  (-90 degrees)
    //   G_yy[1][0] = conj(G_yy[0][1]) = +2i
    //   G_yy[0][0] = G_yy[1][1] = 2
    // The Bendat and Piersol definition, G_xy = E[conj(X) Y], is the conjugate and
    // would give +2i at [0][1]. Getting -2i here is what pins the convention.
    //
    // The cross spectrum is G_yx = E[Y X^H] = H G_xx = [2, 2i]^T, and H1 = G_yx
    // G_xx^-1 must return H with its +90 degree element, not its conjugate.
    let h = MatrixStack::from_vec(1, 2, 1, vec![c(1.0, 0.0), c(0.0, 1.0)]).unwrap();
    let gxx = HermitianStack::from_packed(1, 1, vec![c(2.0, 0.0)]).unwrap();
    let gyy = response_from_drive(&h, &gxx).unwrap();
    assert!(
        (gyy.get(0, 0, 1) - c(0.0, -2.0)).norm() < 1e-15,
        "{:?}",
        gyy.get(0, 0, 1)
    );
    assert!((gyy.get(0, 1, 0) - c(0.0, 2.0)).norm() < 1e-15);
    assert!((gyy.get(0, 0, 0).re - 2.0).abs() < 1e-15);
    assert!((gyy.get(0, 1, 1).re - 2.0).abs() < 1e-15);

    // The same matrix from the definition: average Y Y^H over a fixed ensemble of X.
    let mut rng = Rng(14);
    let k = 256;
    let mut acc = [[ZERO; 2]; 2];
    let mut pxx = 0.0;
    for _ in 0..k {
        let x = rng.cplx();
        let y = [x, c(0.0, 1.0) * x];
        for r in 0..2 {
            for cc in 0..2 {
                acc[r][cc] += y[r] * y[cc].conj();
            }
        }
        pxx += x.norm_sqr();
    }
    let kf = k as f64;
    let gxx_e = HermitianStack::from_packed(1, 1, vec![c(pxx / kf, 0.0)]).unwrap();
    let gyy_e = response_from_drive(&h, &gxx_e).unwrap();
    for r in 0..2 {
        for cc in 0..2 {
            assert!((gyy_e.get(0, r, cc) - acc[r][cc] / kf).norm() < 1e-12);
        }
    }
    assert!(acc[0][1].im < 0.0);

    let gyx = MatrixStack::from_vec(1, 2, 1, vec![c(2.0, 0.0), c(0.0, 2.0)]).unwrap();
    let h1 = h1_frf(&gyx, &gxx, Regularization::None).unwrap();
    assert!((h1.get(0, 0, 0) - c(1.0, 0.0)).norm() < 1e-15);
    assert!(
        (h1.get(0, 1, 0) - c(0.0, 1.0)).norm() < 1e-15,
        "{:?}",
        h1.get(0, 1, 0)
    );
}

#[test]
fn pinv_of_a_hand_computed_column_and_row() {
    // The column [3, 4]^T has the one singular value 5 and pseudo-inverse [3, 4] / 25.
    let col = MatrixStack::from_vec(1, 2, 1, vec![c(3.0, 0.0), c(4.0, 0.0)]).unwrap();
    let (p, rep) = pinv(&col, Regularization::None).unwrap();
    assert_eq!(p.dims(), (1, 1, 2));
    assert!((p.get(0, 0, 0) - c(0.12, 0.0)).norm() < 1e-15);
    assert!((p.get(0, 0, 1) - c(0.16, 0.0)).norm() < 1e-15);
    assert_eq!(rep.k, 1);
    assert!((rep.singular_values[0] - 5.0).abs() < 1e-14);
    assert_eq!(rep.kept, vec![1]);
    assert_eq!(rep.rank, vec![1]);
    assert!((rep.cond_kept[0] - 1.0).abs() < 1e-15);
    // The row [3, 4i] has pseudo-inverse a^H / |a|^2 = [3, -4i]^T / 25 (minimum norm).
    let row = MatrixStack::from_vec(1, 1, 2, vec![c(3.0, 0.0), c(0.0, 4.0)]).unwrap();
    let (p, _) = pinv(&row, Regularization::None).unwrap();
    assert_eq!(p.dims(), (1, 2, 1));
    assert!((p.get(0, 0, 0) - c(0.12, 0.0)).norm() < 1e-15);
    assert!((p.get(0, 1, 0) - c(0.0, -0.16)).norm() < 1e-15);
}

// ---------------------------------------------------------------------------
// Positive semi-definite repair and factors
// ---------------------------------------------------------------------------

#[test]
fn psd_repair_clamps_counts_and_restores_the_diagonal() {
    let mut rng = Rng(4);
    let lams = vec![
        // -1e-4 is below -1e-6 * 2: clamped. -1e-7 is rounding level.
        vec![-1e-4, -1e-7, 0.5, 1.0, 2.0],
        // Two clamped.
        vec![-3e-4, -2e-4, 0.0, 1.0, 1.0],
        // Already positive semi-definite.
        vec![0.0, 0.1, 0.2, 0.3, 0.4],
        // Rounding-level negative only: left as it was.
        vec![-1e-8, 0.1, 0.2, 0.3, 0.4],
    ];
    let g0 = with_eigenvalues(&mut rng, &lams);
    let mut g = g0.clone();
    let rep = psd_repair(&mut g, PSD_CLAMP_REL, PSD_REJECT_REL).unwrap();
    assert_eq!(rep.clamped, vec![1, 2, 0, 0]);
    assert_eq!(rep.total_clamped(), 3);
    assert_eq!(rep.lines_repaired(), 2);
    assert_eq!(rep.worst_line, Some(1));
    assert!(
        (rep.worst_relative + 3e-4).abs() < 1e-9,
        "{}",
        rep.worst_relative
    );

    let e = eigh(&g).unwrap();
    for line in 0..2 {
        let v = &e.values[line * 5..(line + 1) * 5];
        assert!(v[0] >= -1e-12 * v[4], "line {line}: {v:?}");
        for i in 0..5 {
            let (got, want) = (g.get(line, i, i).re, g0.get(line, i, i).re);
            assert!(
                (got - want).abs() <= 1e-12 * want.abs(),
                "line {line} diag {i}"
            );
        }
    }
    // Lines that needed nothing are bit for bit unchanged.
    assert_eq!(g.line(2), g0.line(2));
    assert_eq!(g.line(3), g0.line(3));
}

#[test]
fn psd_repair_rejects_a_large_negative_eigenvalue_naming_the_line_and_changes_nothing() {
    let mut rng = Rng(5);
    let lams = vec![
        vec![0.1, 0.2, 1.0],
        vec![0.1, 0.2, 1.0],
        vec![-0.01, 0.5, 1.0],
        vec![-0.5, 0.2, 1.0],
    ];
    let g0 = with_eigenvalues(&mut rng, &lams);
    let mut g = g0.clone();
    let err = psd_repair(&mut g, PSD_CLAMP_REL, PSD_REJECT_REL).unwrap_err();
    match err {
        LinalgError::NegativeEigenvalue {
            line,
            eigenvalue,
            lambda_max,
            ..
        } => {
            assert_eq!(line, 2);
            assert!((eigenvalue + 0.01).abs() < 1e-12, "{eigenvalue}");
            assert!((lambda_max - 1.0).abs() < 1e-12);
        }
        ref other => panic!("{other:?}"),
    }
    assert!(err.to_string().contains("line 2"), "{err}");
    assert_eq!(g, g0);

    // A negative definite line has no positive lambda_max: refused.
    let mut neg = HermitianStack::zeros(1, 2);
    neg.set(0, 0, 0, c(-1.0, 0.0));
    neg.set(0, 1, 1, c(-1.0, 0.0));
    assert!(matches!(
        psd_repair(&mut neg, PSD_CLAMP_REL, PSD_REJECT_REL),
        Err(LinalgError::NegativeEigenvalue { line: 0, .. })
    ));
    // An all-zero stack is positive semi-definite.
    let mut z = HermitianStack::zeros(2, 3);
    let rep = psd_repair(&mut z, PSD_CLAMP_REL, PSD_REJECT_REL).unwrap();
    assert_eq!(rep.total_clamped(), 0);
    assert_eq!(rep.worst_line, None);
}

#[test]
fn psd_repair_refuses_bad_thresholds_and_non_finite_input() {
    let mut g = HermitianStack::zeros(1, 2);
    for (cl, rj) in [
        (-1e-6, 1e-3),
        (1e-2, 1e-3),
        (f64::NAN, 1e-3),
        (1e-6, f64::INFINITY),
    ] {
        assert!(
            matches!(psd_repair(&mut g, cl, rj), Err(LinalgError::Parameter(_))),
            "{cl} {rj}"
        );
    }
    let mut bad = HermitianStack::zeros(3, 2);
    bad.set(1, 0, 1, c(f64::NAN, 0.0));
    assert!(matches!(
        psd_repair(&mut bad, PSD_CLAMP_REL, PSD_REJECT_REL),
        Err(LinalgError::NonFinite { line: 1, .. })
    ));
}

#[test]
fn the_eigen_square_root_reproduces_g() {
    let mut rng = Rng(8);
    for rank in [24, 5] {
        let g = random_psd(&mut rng, 3, 24, rank);
        let l = sqrt_factor(&g).unwrap();
        assert_eq!(l.dims(), (3, 24, 24));
        for line in 0..3 {
            let ll = mul(l.line(line), &adj(l.line(line), 24, 24), 24, 24, 24);
            let gl = full(&g, line);
            let err = diff(&ll, &gl) / maxabs(&gl);
            assert!(err <= 1e-12, "rank {rank} line {line}: {err:e}");
        }
    }
}

#[test]
fn cholesky_reproduces_a_definite_line_and_refuses_a_singular_one() {
    let mut rng = Rng(9);
    let n = 8;
    let g = random_psd(&mut rng, 3, n, n);
    let l = cholesky_factor(&g).unwrap();
    for line in 0..3 {
        let ll = l.line(line);
        for i in 0..n {
            assert!(
                ll[i * n + i].re > 0.0 && ll[i * n + i].im == 0.0,
                "line {line} diagonal {i}: {:?}",
                ll[i * n + i]
            );
            for j in (i + 1)..n {
                assert_eq!(ll[i * n + j], ZERO, "upper triangle at ({i}, {j})");
            }
        }
        let rec = mul(ll, &adj(ll, n, n), n, n, n);
        let gl = full(&g, line);
        assert!(diff(&rec, &gl) <= 1e-12 * maxabs(&gl));
    }
    // Line 1 replaced by a rank-3 line: refused by name; the eigen root still works.
    let low = random_psd(&mut rng, 1, n, 3);
    let lines: Vec<Vec<C64>> = vec![full(&g, 0), full(&low, 0), full(&g, 2)];
    let gs = from_full_lines(n, &lines);
    match cholesky_factor(&gs) {
        Err(LinalgError::NotPositiveDefinite { line: 1, .. }) => {}
        other => panic!("{other:?}"),
    }
    assert!(sqrt_factor(&gs).is_ok());
}

// ---------------------------------------------------------------------------
// SVD and pseudo-inverse
// ---------------------------------------------------------------------------

#[test]
fn svd_reconstructs_square_tall_and_wide_lines() {
    let mut rng = Rng(10);
    for (m, n) in [(6, 6), (9, 4), (4, 9)] {
        let lines = 3;
        let h = random_stack(&mut rng, lines, m, n);
        let d = svd(&h).unwrap();
        let k = m.min(n);
        assert_eq!(d.k(), k);
        assert_eq!(d.u.dims(), (lines, m, k));
        assert_eq!(d.v.dims(), (lines, n, k));
        for line in 0..lines {
            let s = &d.s[line * k..(line + 1) * k];
            assert!(
                s.windows(2).all(|w| w[0] >= w[1]) && s[k - 1] >= 0.0,
                "{s:?}"
            );
            let mut us = d.u.line(line).to_vec();
            for i in 0..m {
                for j in 0..k {
                    us[i * k + j] *= s[j];
                }
            }
            let rec = mul(&us, &adj(d.v.line(line), n, k), m, k, n);
            assert!(diff(&rec, h.line(line)) <= 1e-12 * maxabs(h.line(line)));
            let uhu = mul(&adj(d.u.line(line), m, k), d.u.line(line), k, m, k);
            let vhv = mul(&adj(d.v.line(line), n, k), d.v.line(line), k, n, k);
            assert!(diff(&uhu, &eye(k)) < 1e-12);
            assert!(diff(&vhv, &eye(k)) < 1e-12);
        }
    }
}

#[test]
fn pinv_satisfies_the_moore_penrose_identities() {
    let mut rng = Rng(11);
    for (m, n) in [(24, 24), (72, 24), (24, 72)] {
        let lines = 2;
        let a = random_stack(&mut rng, lines, m, n);
        let (p, rep) = pinv(&a, Regularization::None).unwrap();
        assert_eq!(p.dims(), (lines, n, m));
        for line in 0..lines {
            // The seeded data are well conditioned; the tolerances below rely on it.
            assert!(rep.cond_overall[line] < 1e5, "{}", rep.cond_overall[line]);
            assert_eq!(rep.kept[line], m.min(n));
            let al = a.line(line);
            let pl = p.line(line);
            let ap = mul(al, pl, m, n, m);
            let pa = mul(pl, al, n, m, n);
            let tol = 1e-9;
            // A A+ A = A
            assert!(
                diff(&mul(&ap, al, m, m, n), al) <= tol * maxabs(al),
                "{m}x{n} (1)"
            );
            // A+ A A+ = A+
            assert!(
                diff(&mul(&pa, pl, n, n, m), pl) <= tol * maxabs(pl),
                "{m}x{n} (2)"
            );
            // (A A+)^H = A A+
            assert!(diff(&adj(&ap, m, m), &ap) <= tol, "{m}x{n} (3)");
            // (A+ A)^H = A+ A
            assert!(diff(&adj(&pa, n, n), &pa) <= tol, "{m}x{n} (4)");
        }
    }
}

#[test]
fn truncated_pinv_drops_the_known_small_singular_value() {
    // H = U diag(1, 0.5, 1e-6) V^H, 4 x 3, with random unitary U and V.
    let mut rng = Rng(12);
    let u = random_unitary(&mut rng, 1, 4);
    let v = random_unitary(&mut rng, 1, 3);
    let s = [1.0, 0.5, 1e-6];
    let (m, n) = (4, 3);
    let mut hd = vec![ZERO; m * n];
    for i in 0..m {
        for j in 0..n {
            for k in 0..3 {
                hd[i * n + j] += u.get(0, i, k) * s[k] * v.get(0, j, k).conj();
            }
        }
    }
    let h = MatrixStack::from_vec(1, m, n, hd).unwrap();
    // The expected inverse for given filter factors: V diag(f) U^H.
    let expected = |f: [f64; 3]| -> Vec<C64> {
        let mut out = vec![ZERO; n * m];
        for j in 0..n {
            for i in 0..m {
                for k in 0..3 {
                    out[j * m + i] += v.get(0, j, k) * f[k] * u.get(0, i, k).conj();
                }
            }
        }
        out
    };

    let (p, rep) = pinv(&h, Regularization::Truncated { rcond: 1e-3 }).unwrap();
    assert_eq!(rep.kept, vec![2]);
    assert_eq!(rep.rank, vec![3]);
    assert!((rep.cond_kept[0] - 2.0).abs() < 1e-9);
    assert!((rep.cond_overall[0] / 1e6 - 1.0).abs() < 1e-6);
    assert!(diff(p.line(0), &expected([1.0, 2.0, 0.0])) < 1e-10);

    // Without truncation the 1e-6 value is inverted.
    let (p_all, rep_all) = pinv(&h, Regularization::None).unwrap();
    assert_eq!(rep_all.kept, vec![3]);
    assert!(maxabs(p_all.line(0)) > 1e5);

    // Keep-rank 1 keeps only the largest.
    let (_, r1) = pinv(&h, Regularization::KeepRank { k: 1 }).unwrap();
    assert_eq!(r1.kept, vec![1]);
    assert!((r1.cond_kept[0] - 1.0).abs() < 1e-12);

    // Tikhonov filters each value by s / (s^2 + mu^2), mu = lambda_rel * s_max.
    let mu = 1e-3;
    let (pt, rt) = pinv(&h, Regularization::Tikhonov { lambda_rel: mu }).unwrap();
    assert_eq!(rt.kept, vec![3]);
    let f = s.map(|x| x / (x * x + mu * mu));
    assert!(diff(pt.line(0), &expected(f)) < 1e-9);
}

// ---------------------------------------------------------------------------
// Products, transformations, identification
// ---------------------------------------------------------------------------

#[test]
fn congruence_keeps_psd_and_matches_the_naive_product() {
    let mut rng = Rng(7);
    let (lines, n) = (4, 6);
    // Rank 3: semi-definite, not definite.
    let g = random_psd(&mut rng, lines, n, 3);
    for m in [2, 6, 9] {
        let t = random_stack(&mut rng, lines, m, n);
        let r = congruence(&t, &g).unwrap();
        assert_eq!((r.lines(), r.n()), (lines, m));
        let e = eigh(&r).unwrap();
        for line in 0..lines {
            let tl = t.line(line);
            let want = mul(&mul(tl, &full(&g, line), m, n, n), &adj(tl, m, n), m, n, m);
            let scale = maxabs(&want);
            assert!(
                diff(&full(&r, line), &want) <= 1e-12 * scale,
                "m {m} line {line}"
            );
            let v = &e.values[line * m..(line + 1) * m];
            assert!(v[0] >= -1e-12 * v[m - 1], "m {m} line {line}: {v:?}");
        }
    }
    // One transformation line applies to every line.
    let t1 = random_stack(&mut rng, 1, 2, n);
    let r1 = congruence(&t1, &g).unwrap();
    for line in 0..lines {
        let tl = t1.line(0);
        let want = mul(&mul(tl, &full(&g, line), 2, n, n), &adj(tl, 2, n), 2, n, 2);
        assert!(diff(&full(&r1, line), &want) <= 1e-12 * maxabs(&want));
    }
}

#[test]
fn frf_transform_matches_the_naive_product() {
    let mut rng = Rng(16);
    let lines = 3;
    let h = random_stack(&mut rng, lines, 5, 4);
    let tr = random_stack(&mut rng, 1, 2, 5);
    let td = random_stack(&mut rng, lines, 4, 3);
    let out = frf_transform(&tr, &h, Some(&td)).unwrap();
    assert_eq!(out.dims(), (lines, 2, 3));
    let out2 = frf_transform(&tr, &h, None).unwrap();
    assert_eq!(out2.dims(), (lines, 2, 4));
    for line in 0..lines {
        let th = mul(tr.line(0), h.line(line), 2, 5, 4);
        let want = mul(&th, td.line(line), 2, 4, 3);
        assert!(diff(out.line(line), &want) <= 1e-12 * maxabs(&want));
        assert!(diff(out2.line(line), &th) <= 1e-12 * maxabs(&th));
    }
}

#[test]
fn h1_recovers_a_known_frf_from_a_diagonal_drive() {
    let mut rng = Rng(13);
    for (nc, nd) in [(6, 3), (72, 24)] {
        let lines = 2;
        let h = random_stack(&mut rng, lines, nc, nd);
        let mut gxx = HermitianStack::zeros(lines, nd);
        for line in 0..lines {
            for j in 0..nd {
                gxx.set(line, j, j, c(0.5 + rng.uniform(), 0.0));
            }
        }
        let mut gyx_data = Vec::new();
        for line in 0..lines {
            gyx_data.extend(mul(h.line(line), &full(&gxx, line), nc, nd, nd));
        }
        let gyx = MatrixStack::from_vec(lines, nc, nd, gyx_data).unwrap();
        let h1 = h1_frf(&gyx, &gxx, Regularization::None).unwrap();
        for line in 0..lines {
            let err = diff(h1.line(line), h.line(line)) / maxabs(h.line(line));
            assert!(err <= 1e-13, "{nc}x{nd} line {line}: {err:e}");
        }
    }
}

#[test]
fn multiple_coherence_is_one_without_noise_and_falls_with_independent_noise() {
    let mut rng = Rng(14);
    let (lines, nc, nd) = (3, 5, 3);
    let mut h = random_stack(&mut rng, lines, nc, nd);
    // Control 4 on line 2 sees nothing: its response PSD is zero.
    for j in 0..nd {
        h.set(2, 4, j, ZERO);
    }
    let gxx = random_psd(&mut rng, lines, nd, nd);
    let gyy = response_from_drive(&h, &gxx).unwrap();
    let mut gyx_data = Vec::new();
    for line in 0..lines {
        gyx_data.extend(mul(h.line(line), &full(&gxx, line), nc, nd, nd));
    }
    let gyx = MatrixStack::from_vec(lines, nc, nd, gyx_data).unwrap();
    let coh = multiple_coherence(&gyy, &gyx, &gxx, Regularization::None).unwrap();
    assert_eq!(coh.len(), lines * nc);
    for line in 0..lines {
        for i in 0..nc {
            let g2 = coh[line * nc + i];
            if line == 2 && i == 4 {
                assert_eq!(g2, 0.0);
            } else {
                assert!((g2 - 1.0).abs() < 1e-10, "line {line} control {i}: {g2}");
            }
        }
    }
    // Independent noise N = diag(r_i S_ii) on the response: gamma_i^2 = 1 / (1 + r_i).
    let mut noisy = gyy.clone();
    for line in 0..lines {
        for i in 0..nc {
            let s = gyy.get(line, i, i).re;
            let add = if s > 0.0 {
                0.25 * (i + 1) as f64 * s
            } else {
                0.3
            };
            noisy.set(line, i, i, c(s + add, 0.0));
        }
    }
    let coh_n = multiple_coherence(&noisy, &gyx, &gxx, Regularization::None).unwrap();
    for line in 0..lines {
        for i in 0..nc {
            let g2 = coh_n[line * nc + i];
            let want = if line == 2 && i == 4 {
                0.0
            } else {
                1.0 / (1.0 + 0.25 * (i + 1) as f64)
            };
            assert!(
                (g2 - want).abs() < 1e-10,
                "line {line} control {i}: {g2} vs {want}"
            );
            assert!(g2 < coh[line * nc + i] || g2 == 0.0);
        }
    }
}

#[test]
fn projection_achievable_returns_the_reference_for_a_square_plant_and_projects_otherwise() {
    let mut rng = Rng(15);
    // Square and full rank: P = I.
    let h = random_stack(&mut rng, 2, 4, 4);
    let gref = random_psd(&mut rng, 2, 4, 4);
    let a = projection_achievable(&h, &gref, Regularization::None).unwrap();
    for line in 0..2 {
        let want = full(&gref, line);
        assert!(diff(&full(&a, line), &want) <= 1e-10 * maxabs(&want));
    }
    // More controls than drives: the achievable response is what the open-loop
    // pseudo-inverse drive produces, H (H^+ G_ref H^+^H) H^H, and it is a projection.
    let (nc, nd) = (6, 3);
    let h = random_stack(&mut rng, 2, nc, nd);
    let gref = random_psd(&mut rng, 2, nc, nc);
    let a = projection_achievable(&h, &gref, Regularization::None).unwrap();
    let (hp, _) = pinv(&h, Regularization::None).unwrap();
    let drive = congruence(&hp, &gref).unwrap();
    let y = response_from_drive(&h, &drive).unwrap();
    let again = projection_achievable(&h, &a, Regularization::None).unwrap();
    let e = eigh(&a).unwrap();
    for line in 0..2 {
        let scale = maxabs(&full(&gref, line));
        assert!(diff(&full(&a, line), &full(&y, line)) <= 1e-10 * scale);
        assert!(diff(&full(&again, line), &full(&a, line)) <= 1e-10 * scale);
        let v = &e.values[line * nc..(line + 1) * nc];
        let big = v.iter().filter(|&&x| x > 1e-10 * v[nc - 1]).count();
        assert!(big <= nd, "line {line}: {v:?}");
        // and it differs from the reference, which has full rank
        assert!(diff(&full(&a, line), &full(&gref, line)) > 1e-3 * scale);
    }
}

// ---------------------------------------------------------------------------
// Scalars, masks, errors
// ---------------------------------------------------------------------------

#[test]
fn condition_trace_diagonals_db_error_and_band_mask() {
    let mut h = MatrixStack::zeros(2, 3, 3);
    h.set(0, 0, 0, c(0.0, 4.0));
    h.set(0, 1, 1, c(-2.0, 0.0));
    h.set(0, 2, 2, c(1.0, 0.0));
    let k = condition_numbers(&h).unwrap();
    assert!((k[0] - 4.0).abs() < 1e-12, "{k:?}");
    assert!(k[1].is_infinite());

    let mut g = HermitianStack::zeros(2, 2);
    g.set(0, 0, 0, c(1.0, 0.0));
    g.set(0, 1, 1, c(2.0, 0.0));
    g.set(1, 0, 0, c(3.0, 0.0));
    g.set(1, 1, 1, c(4.0, 0.0));
    g.set(1, 0, 1, c(1.0, 1.0));
    assert_eq!(trace(&g), vec![3.0, 7.0]);
    assert_eq!(diagonals(&g), vec![1.0, 2.0, 3.0, 4.0]);

    let db = db_error(
        &[2.0, 1.0, 0.5, 0.0, 1.0, -1.0],
        &[1.0, 1.0, 1.0, 1.0, 0.0, 1.0],
    )
    .unwrap();
    let ten_log_2 = 10.0 * 2f64.log10();
    assert!((db[0] - ten_log_2).abs() < 1e-12);
    assert_eq!(db[1], 0.0);
    assert!((db[2] + ten_log_2).abs() < 1e-12);
    assert_eq!(db[3], f64::NEG_INFINITY);
    assert!(db[4].is_nan() && db[5].is_nan());

    assert_eq!(
        band_mask(6, (1, 4)).unwrap(),
        vec![false, true, true, true, false, false]
    );
    assert!(band_mask(6, (4, 4)).is_err());
    assert!(band_mask(6, (2, 7)).is_err());
    let mut gz = g.clone();
    zero_out_of_band(&mut gz, (0, 1)).unwrap();
    assert_eq!(gz.line(0), g.line(0));
    assert!(gz.line(1).iter().all(|v| *v == ZERO));
}

#[test]
fn shape_mismatches_are_named_errors_not_panics() {
    fn is_shape<T: std::fmt::Debug>(r: Result<T, LinalgError>, needle: &str) {
        match r {
            Err(LinalgError::Shape(e)) => assert!(e.0.contains(needle), "{e} lacks {needle}"),
            other => panic!("expected a shape error naming {needle}, got {other:?}"),
        }
    }
    let g3 = HermitianStack::zeros(2, 3);
    let g2 = HermitianStack::zeros(2, 2);
    let h32 = MatrixStack::zeros(2, 3, 2);
    let empty_h = HermitianStack::zeros(2, 0);
    let empty_m = MatrixStack::zeros(2, 0, 3);
    is_shape(eigh(&empty_h), "eigh");
    is_shape(sqrt_factor(&empty_h), "sqrt_factor");
    is_shape(cholesky_factor(&empty_h), "cholesky_factor");
    is_shape(psd_repair(&mut empty_h.clone(), 1e-6, 1e-3), "psd_repair");
    is_shape(svd(&empty_m), "svd");
    is_shape(pinv(&empty_m, Regularization::None), "pinv");
    is_shape(condition_numbers(&empty_m), "condition_numbers");
    is_shape(congruence(&h32, &g3), "spectral matrix");
    is_shape(
        congruence(&MatrixStack::zeros(3, 3, 2), &g2),
        "transformation",
    );
    is_shape(
        response_from_drive(&MatrixStack::zeros(1, 3, 2), &g2),
        "frf",
    );
    is_shape(response_from_drive(&h32, &g3), "drive SDM");
    is_shape(
        projection_achievable(&h32, &g2, Regularization::None),
        "reference",
    );
    is_shape(
        projection_achievable(&h32, &HermitianStack::zeros(3, 3), Regularization::None),
        "reference",
    );
    is_shape(
        frf_transform(&MatrixStack::zeros(1, 2, 4), &h32, None),
        "response transformation",
    );
    is_shape(
        frf_transform(
            &MatrixStack::zeros(1, 2, 3),
            &h32,
            Some(&MatrixStack::zeros(1, 3, 2)),
        ),
        "drive transformation",
    );
    is_shape(h1_frf(&h32, &g3, Regularization::None), "drive SDM");
    is_shape(
        multiple_coherence(&g2, &h32, &g2, Regularization::None),
        "response SDM",
    );
    is_shape(
        multiple_coherence(&g3, &h32, &g3, Regularization::None),
        "drive SDM",
    );
    is_shape(db_error(&[1.0], &[1.0, 2.0]), "db_error");
    is_shape(band_mask(4, (2, 2)), "band");
    for reg in [
        Regularization::Tikhonov { lambda_rel: -1.0 },
        Regularization::Truncated { rcond: f64::NAN },
        Regularization::KeepRank { k: 0 },
    ] {
        assert!(
            matches!(pinv(&h32, reg), Err(LinalgError::Parameter(_))),
            "{reg:?}"
        );
    }
    // The law boundary carries the message.
    let le: crate::law::LawError = congruence(&h32, &g3).unwrap_err().into();
    assert!(le.0.contains("spectral matrix"), "{le}");
}

#[test]
fn non_finite_input_is_a_named_error() {
    let mut g = HermitianStack::zeros(3, 2);
    for line in 0..3 {
        g.set(line, 0, 0, ONE);
        g.set(line, 1, 1, ONE);
    }
    g.set(1, 0, 0, c(f64::INFINITY, 0.0));
    for r in [
        eigh(&g).map(|_| ()),
        sqrt_factor(&g).map(|_| ()),
        cholesky_factor(&g).map(|_| ()),
    ] {
        assert!(
            matches!(r, Err(LinalgError::NonFinite { line: 1, .. })),
            "{r:?}"
        );
    }
    let mut h = MatrixStack::zeros(3, 2, 2);
    h.set(2, 1, 0, c(0.0, f64::NAN));
    for r in [
        svd(&h).map(|_| ()),
        pinv(&h, Regularization::None).map(|_| ()),
        condition_numbers(&h).map(|_| ()),
    ] {
        match r {
            Err(e @ LinalgError::NonFinite { line: 2, .. }) => {
                assert!(e.to_string().contains("line 2"))
            }
            other => panic!("{other:?}"),
        }
    }
}
