//! The modal fit `followed_poles` is built on: poles by a poly-reference
//! least-squares frequency-domain estimate read over many orders, shapes by a
//! least-squares fit in the frequency domain with real rank-one residues,
//! after a known pure delay is taken out of the identification.
//!
//! The steps, each on the identification's lines inside the test's band:
//!
//! 1. **The delay.** A real residue cannot hold the phase a pure delay turns
//!    the response by, so the identification is first turned back by the delay
//!    it is known to hold, `H(f) exp(j 2 pi f tau)`, and the residues found are
//!    turned by it again at each mode's own frequency, `A exp(-j 2 pi f_r tau)`.
//!    A fit of a delayed identification without this puts the residues wrong
//!    even where the poles are right.
//! 2. **Poles.** The band is cut into fit ranges half an octave wide, each
//!    widened a quarter octave either side. In each range a poly-reference
//!    least-squares estimate (a right matrix-fraction model in `z`, the
//!    denominator's coefficients from the reduced normal equations) is solved
//!    at every order from 12 to 24; its poles are the eigenvalues of the
//!    coefficients' companion matrix, and its participation vectors the last
//!    block of their eigenvectors. Poles that are stable at the orders are kept:
//!    clustered over the orders, highest first (within 1 % in frequency, 25 %
//!    in damping, participation MAC 0.8, one member an order), each cluster
//!    kept when it holds at least half the orders, its member of median
//!    frequency taken. Two kept poles closer than a tenth of their decay rate,
//!    with the participation MAC, are one pole kept twice (a mode split over
//!    two clusters), and only the one held by more orders is kept. A range
//!    keeps the poles in its own central part.
//! 3. **Shapes.** With the poles and the participations (made real) held, each
//!    control's shape and a background of `1`, `(fc/f)^2` and `(f/fc)^2` are
//!    solved by linear least squares over the band (`fc` the band's geometric
//!    centre); the residue is the outer product of shape and participation.
//! 4. **Small modes out.** A mode under a tenth of the identification within
//!    three half-power bandwidths of itself is dropped and the shapes solved
//!    again, at most three times.
//! 5. **Kept.** The modes between 0.9 of the band's low end and 1.1 of its high
//!    end with damping above 1e-4 and under 0.2.
//!
//! Sources, by author, title and venue: B. Peeters, H. Van der Auweraer, P.
//! Guillaume and J. Leuridan, "The PolyMAX frequency-domain method: a new
//! standard for modal parameter estimation?", Shock and Vibration 11 (2004)
//! (the poly-reference least-squares complex frequency-domain estimate and its
//! reduced normal equations); P. Guillaume, P. Verboven, S. Vanlanduit, H. Van
//! der Auweraer and B. Peeters, "A poly-reference implementation of the
//! least-squares complex frequency-domain estimator", IMAC XXI (2003); and the
//! least-squares frequency-domain shape fit with the poles held, as in D. J.
//! Ewins, "Modal Testing: Theory, Practice and Application", 2nd ed. (2000).
//! The order-clustering of stable poles and the settings above were chosen in
//! a study of this fit for the MIMO Sine benchmark and are written here from
//! that study's description.
//!
//! The work is fixed by the settings (13 orders a range, one shape solve and at
//! most three more), so the fit's time is bounded by the band and the counts.

use crate::modal::{q_mode, ModalForm};
use crate::setup::Identification;
use crate::C64;
use faer::linalg::solvers::{Eigen, PartialPivLu, Qr, Solve, SolveLstsq};
use faer::Mat;
use std::f64::consts::TAU;

const ZERO: C64 = C64::new(0.0, 0.0);

/// The pLSCF orders solved in each fit range.
pub const ORDERS: std::ops::RangeInclusive<usize> = 12..=24;

/// A mode under this share of the identification near itself is dropped.
pub const MIN_SHARE: f64 = 0.1;

/// The settings of the order clustering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clustering {
    /// Frequency tolerance, a fraction of the cluster's frequency.
    pub df: f64,
    /// Damping tolerance, a fraction of the cluster's damping.
    pub dz: f64,
    /// Least participation MAC.
    pub mac: f64,
    /// Least share of the orders a kept cluster holds.
    pub share: f64,
    /// Poles with damping at or above this are not clustered.
    pub zmax: f64,
    /// Two kept poles closer than this share of their decay rate (`|Re s|`),
    /// with the participation MAC, are one pole kept twice.
    pub same: f64,
}

impl Default for Clustering {
    fn default() -> Self {
        Clustering {
            df: 0.01,
            dz: 0.25,
            mac: 0.8,
            share: 0.5,
            zmax: 0.2,
            same: 0.1,
        }
    }
}

/// The lines of the band: their frequencies and responses (`lines x n x m`).
struct Band {
    g: Vec<f64>,
    h: Vec<C64>,
    n: usize,
    m: usize,
}

impl Band {
    fn of(id: &Identification, band: (f64, f64), tau: f64) -> Band {
        let (_, n, m) = id.response.dims();
        let mut g = Vec::new();
        let mut h = Vec::new();
        for (k, &f) in id.grid_hz.iter().enumerate() {
            if f >= band.0 && f <= band.1 {
                g.push(f);
                let turn = C64::from_polar(1.0, TAU * f * tau);
                h.extend(id.response.line(k).iter().map(|v| v * turn));
            }
        }
        Band { g, h, n, m }
    }

    fn select(&self, lo: f64, hi: f64) -> Band {
        let e = self.n * self.m;
        let mut g = Vec::new();
        let mut h = Vec::new();
        for (k, &f) in self.g.iter().enumerate() {
            if f >= lo && f <= hi {
                g.push(f);
                h.extend_from_slice(&self.h[k * e..(k + 1) * e]);
            }
        }
        Band {
            g,
            h,
            n: self.n,
            m: self.m,
        }
    }
}

/// The poles (`s`, rad/s) and participation vectors (`poles x m`, each scaled
/// to a largest magnitude of one) of one order.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderPoles {
    /// The order.
    pub order: usize,
    /// The poles kept: stable, with a frequency on the lines.
    pub poles: Vec<C64>,
    /// Their participation vectors, pole after pole.
    pub participation: Vec<C64>,
}

fn freq_damp(lam: C64) -> (f64, f64) {
    (lam.im / TAU, -lam.re / lam.norm())
}

/// The pLSCF estimate over the lines `g`, `h` (`lines x n x m`) at each order,
/// frequencies taken from `f0`. Orders whose reduced system is singular are
/// left out.
fn plscf_orders(b: &Band, orders: &[usize], f0: f64) -> Vec<OrderPoles> {
    let (l, n, m) = (b.g.len(), b.n, b.m);
    if l < 2 {
        return Vec::new();
    }
    let dt = 1.0 / (2.0 * (b.g[l - 1] - f0));
    let pmax = orders.iter().copied().max().unwrap_or(0);
    let z: Vec<C64> =
        b.g.iter()
            .map(|&f| C64::from_polar(1.0, TAU * (f - f0) * dt))
            .collect();
    // Powers z^d for d in 0..=2 pmax (|z| = 1, so z^-d is the conjugate).
    let nd = 2 * pmax + 1;
    let mut zp = vec![ZERO; l * nd];
    for (k, zk) in z.iter().enumerate() {
        let mut v = C64::new(1.0, 0.0);
        for d in 0..nd {
            zp[k * nd + d] = v;
            v *= zk;
        }
    }
    let zpow = |k: usize, d: isize| -> C64 {
        if d >= 0 {
            zp[k * nd + d as usize]
        } else {
            zp[k * nd + (-d) as usize].conj()
        }
    };
    let np1 = pmax + 1;
    // R[r][s] = Re sum z^(s-r), shared by every control (unit weights).
    let rsum: Vec<f64> = (0..=2 * np1)
        .map(|di| {
            let d = di as isize - np1 as isize;
            (0..l).map(|k| zpow(k, d).re).sum()
        })
        .collect();
    let rr = |r: usize, s: usize| rsum[(s as isize - r as isize + np1 as isize) as usize];
    // Per control: g1[d][b] = sum z^d H_b and g2[d][a][b] = sum z^d conj(H_a) H_b.
    let span = 2 * pmax + 1;
    let mut g1 = vec![ZERO; n * span * m];
    let mut g2 = vec![ZERO; n * span * m * m];
    for o in 0..n {
        for k in 0..l {
            let hl = &b.h[(k * n + o) * m..(k * n + o + 1) * m];
            for di in 0..span {
                let d = di as isize - pmax as isize;
                let w = zpow(k, d);
                let base1 = (o * span + di) * m;
                for (bb, hb) in hl.iter().enumerate() {
                    g1[base1 + bb] += w * hb;
                }
                let base2 = (o * span + di) * m * m;
                for (a, ha) in hl.iter().enumerate() {
                    let wa = w * ha.conj();
                    for (bb, hb) in hl.iter().enumerate() {
                        g2[base2 + a * m + bb] += wa * hb;
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut sorted: Vec<usize> = orders.to_vec();
    sorted.sort_unstable();
    for &p in &sorted {
        let k = m * (p + 1);
        let rp = Mat::<f64>::from_fn(p + 1, p + 1, &rr);
        let lu = PartialPivLu::new(rp.as_ref());
        let mut mm = Mat::<f64>::zeros(k, k);
        for o in 0..n {
            let di = |r: usize, s: usize| (s as isize - r as isize + pmax as isize) as usize;
            // S[r][(s, b)] = -Re g1[s - r][b]; T[(r, a)][(s, b)] = Re g2[s - r][a][b].
            let sp = Mat::<f64>::from_fn(p + 1, k, |r, c| {
                let (s, bb) = (c / m, c % m);
                -g1[(o * span + di(r, s)) * m + bb].re
            });
            let x = lu.solve(sp.as_ref());
            for i in 0..k {
                let (r, a) = (i / m, i % m);
                for j in 0..k {
                    let (s, bb) = (j / m, j % m);
                    let t = g2[(o * span + di(r, s)) * m * m + a * m + bb].re;
                    let mut st = 0.0;
                    for q in 0..=p {
                        st += sp[(q, i)] * x[(q, j)];
                    }
                    mm[(i, j)] += t - st;
                }
            }
        }
        let mp = m * p;
        let maa = Mat::<f64>::from_fn(mp, mp, |i, j| mm[(i, j)]);
        let mab = Mat::<f64>::from_fn(mp, m, |i, j| mm[(i, mp + j)]);
        let alpha = PartialPivLu::new(maa.as_ref()).solve(mab.as_ref());
        if (0..mp).any(|i| (0..m).any(|j| !alpha[(i, j)].is_finite())) {
            continue;
        }
        // The companion: first block row -alpha_(p-1-r)^T (alpha_a negated), identity below.
        let cc = Mat::<f64>::from_fn(mp, mp, |i, j| {
            if i < m {
                let (r, c) = (j / m, j % m);
                let q = p - 1 - r;
                // -alphas[q]^T[i][c] = -(-alpha_a)[q m + c][i] = alpha_a[q m + c][i]
                alpha[(q * m + c, i)]
            } else if j + m == i {
                1.0
            } else {
                0.0
            }
        });
        let Ok(e) = Eigen::<f64>::new_from_real(cc.as_ref()) else {
            continue;
        };
        let s = e.S().column_vector();
        let u = e.U();
        let mut poles = Vec::new();
        let mut part = Vec::new();
        for i in 0..mp {
            let zz: C64 = s[i];
            let lam = zz.ln() / dt + C64::new(0.0, TAU * f0);
            let f = lam.im / TAU;
            if !(lam.re < 0.0 && f >= b.g[0] && f <= b.g[l - 1]) {
                continue;
            }
            let v: Vec<C64> = (0..m).map(|j| u[(mp - m + j, i)]).collect();
            let big = v.iter().map(|c| c.norm()).fold(0.0, f64::max).max(1e-300);
            poles.push(lam);
            part.extend(v.iter().map(|c| c / big));
        }
        out.push(OrderPoles {
            order: p,
            poles,
            participation: part,
        });
    }
    out
}

fn mac(a: &[C64], b: &[C64]) -> f64 {
    let ab: C64 = a.iter().zip(b).map(|(x, y)| x.conj() * y).sum();
    let aa: C64 = a.iter().map(|x| x.conj() * x).sum();
    let bb: C64 = b.iter().map(|x| x.conj() * x).sum();
    ab.norm_sqr() / (aa * bb).re.max(1e-300)
}

/// Poles stable over the orders (see the module notes): the representative
/// pole of each kept cluster and its participation vector.
pub fn cluster_orders(poles: &[OrderPoles], m: usize, c: Clustering) -> (Vec<C64>, Vec<C64>) {
    struct Cl {
        f0: f64,
        z0: f64,
        l0: Vec<C64>,
        members: Vec<(usize, C64, Vec<C64>)>,
    }
    let mut orders: Vec<&OrderPoles> = poles.iter().collect();
    orders.sort_by_key(|a| std::cmp::Reverse(a.order));
    let mut clusters: Vec<Cl> = Vec::new();
    for op in &orders {
        let p = op.order;
        for (i, &lam) in op.poles.iter().enumerate() {
            let (f, z) = freq_damp(lam);
            if !(z > 0.0 && z < c.zmax) {
                continue;
            }
            let li = &op.participation[i * m..(i + 1) * m];
            let mut best: Option<usize> = None;
            let mut bd = f64::INFINITY;
            for (ci, cl) in clusters.iter().enumerate() {
                if cl.members.iter().any(|(o, _, _)| *o == p) {
                    continue;
                }
                let tol = c.df * cl.f0;
                if (f - cl.f0).abs() <= tol
                    && (z - cl.z0).abs() <= c.dz * cl.z0
                    && mac(li, &cl.l0) >= c.mac
                {
                    let d = (f - cl.f0).abs();
                    if d < bd {
                        best = Some(ci);
                        bd = d;
                    }
                }
            }
            match best {
                None => clusters.push(Cl {
                    f0: f,
                    z0: z,
                    l0: li.to_vec(),
                    members: vec![(p, lam, li.to_vec())],
                }),
                Some(ci) => clusters[ci].members.push((p, lam, li.to_vec())),
            }
        }
    }
    let total = orders.len() as f64;
    // The kept clusters' representatives: the orders held, the pole and its
    // participation.
    let mut kept: Vec<(usize, C64, &[C64])> = Vec::new();
    for cl in &clusters {
        let mut held: Vec<usize> = cl.members.iter().map(|(o, _, _)| *o).collect();
        held.sort_unstable();
        held.dedup();
        if (held.len() as f64) >= c.share * total {
            let mut idx: Vec<usize> = (0..cl.members.len()).collect();
            idx.sort_by(|&a, &b| cl.members[a].1.im.total_cmp(&cl.members[b].1.im));
            let k = idx[idx.len() / 2];
            kept.push((held.len(), cl.members[k].1, &cl.members[k].2));
        }
    }
    // One pole kept twice. A cluster takes one pole an order, so where a
    // spurious pole near a mode reaches the mode's cluster first at some
    // order, the mode's own pole there starts a second cluster at the same
    // frequency; which of the two each later order joins then turns on
    // rounding, and both can be held by enough orders to be kept. Two kept
    // poles closer than `same` of their decay rate, with the participation
    // MAC, are that one pole: the one held by more orders is kept (the
    // earlier cluster on a tie). Two modes of a close pair are a good part of
    // a bandwidth apart, so stay two.
    let same_pole = |a: &(usize, C64, &[C64]), b: &(usize, C64, &[C64])| {
        (a.1 - b.1).norm() < c.same * a.1.re.abs().min(b.1.re.abs()) && mac(a.2, b.2) >= c.mac
    };
    let mut out: Vec<usize> = Vec::new();
    for k in 0..kept.len() {
        match out.iter().position(|&j| same_pole(&kept[j], &kept[k])) {
            None => out.push(k),
            Some(q) => {
                if kept[k].0 > kept[out[q]].0 {
                    out[q] = k;
                }
            }
        }
    }
    out.sort_unstable();
    let mut lam_out = Vec::new();
    let mut l_out = Vec::new();
    for &k in &out {
        lam_out.push(kept[k].1);
        l_out.extend_from_slice(kept[k].2);
    }
    (lam_out, l_out)
}

/// The fit ranges over `band`: `(lo, hi, keep_lo, keep_hi)`, each half an
/// octave wide plus a quarter octave either side, keeping its own poles in
/// `[keep_lo, keep_hi)` (the last range to infinity).
pub fn fit_ranges(band: (f64, f64)) -> Vec<(f64, f64, f64, f64)> {
    let (width, overlap) = (0.5f64, 0.25f64);
    let mut out = Vec::new();
    let mut lo = band.0;
    while lo < band.1 {
        let hi = (lo * 2f64.powf(width)).min(band.1);
        out.push((
            band.0.max(lo * 2f64.powf(-overlap)),
            band.1.min(hi * 2f64.powf(overlap)),
            lo,
            if hi < band.1 { hi } else { f64::INFINITY },
        ));
        lo = hi;
    }
    out
}

fn poles_over_ranges(b: &Band, band: (f64, f64)) -> (Vec<C64>, Vec<C64>) {
    let m = b.m;
    let orders: Vec<usize> = ORDERS.collect();
    let mut lam = Vec::new();
    let mut part = Vec::new();
    for (lo, hi, klo, khi) in fit_ranges(band) {
        let sub = b.select(lo, hi);
        let per = plscf_orders(&sub, &orders, lo);
        let (lr, pr) = cluster_orders(&per, m, Clustering::default());
        for (i, l) in lr.iter().enumerate() {
            let f = l.im / TAU;
            if f >= klo && f < khi {
                lam.push(*l);
                part.extend_from_slice(&pr[i * m..(i + 1) * m]);
            }
        }
    }
    let mut idx: Vec<usize> = (0..lam.len()).collect();
    idx.sort_by(|&a, &c| lam[a].im.total_cmp(&lam[c].im));
    let lam_s = idx.iter().map(|&i| lam[i]).collect();
    let part_s = idx
        .iter()
        .flat_map(|&i| part[i * m..(i + 1) * m].to_vec())
        .collect();
    (lam_s, part_s)
}

/// A fit: frequencies, dampings and residues (`modes x n x m`).
struct Fit {
    f: Vec<f64>,
    z: Vec<f64>,
    a: Vec<C64>,
}

/// Shapes by least squares with the poles and real participations held.
fn shapes(b: &Band, lam: &[C64], part: &[C64]) -> Result<Fit, String> {
    let (l, n, m) = (b.g.len(), b.n, b.m);
    let kk = lam.len();
    let (f, z): (Vec<f64>, Vec<f64>) = lam.iter().map(|&x| freq_damp(x)).unzip();
    // Real participations: each turned so its largest element is real and
    // positive, its real part taken, scaled to unit norm.
    let mut lr = vec![0.0f64; kk * m];
    for k in 0..kk {
        let v = &part[k * m..(k + 1) * m];
        let mut j0 = 0;
        for (j, c) in v.iter().enumerate() {
            if c.norm() > v[j0].norm() {
                j0 = j;
            }
        }
        let turn = C64::from_polar(1.0, -v[j0].arg());
        let row: Vec<f64> = v.iter().map(|c| (turn * c).re).collect();
        let nr = row.iter().map(|x| x * x).sum::<f64>().sqrt().max(1e-300);
        for j in 0..m {
            lr[k * m + j] = row[j] / nr;
        }
    }
    let fc = (b.g[0] * b.g[l - 1]).sqrt();
    let cols = kk + 3 * m;
    let rows = 2 * l * m;
    let mut d = Mat::<f64>::zeros(rows, cols);
    for li in 0..l {
        let g = b.g[li];
        let phi = [1.0, (fc / g).powi(2), (g / fc).powi(2)];
        for k in 0..kk {
            let q = q_mode(g, f[k], z[k]);
            for j in 0..m {
                let v = q * lr[k * m + j];
                d[(li * m + j, k)] = v.re;
                d[(l * m + li * m + j, k)] = v.im;
            }
        }
        for (bi, ph) in phi.iter().enumerate() {
            for j in 0..m {
                d[(li * m + j, kk + bi * m + j)] = *ph;
            }
        }
    }
    let mut y = Mat::<f64>::zeros(rows, n);
    for li in 0..l {
        for i in 0..n {
            for j in 0..m {
                let h = b.h[(li * n + i) * m + j];
                y[(li * m + j, i)] = h.re;
                y[(l * m + li * m + j, i)] = h.im;
            }
        }
    }
    let x = Qr::new(d.as_ref()).solve_lstsq(y.as_ref());
    let mut a = vec![ZERO; kk * n * m];
    for k in 0..kk {
        let row = &lr[k * m..(k + 1) * m];
        let s = row.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
        for i in 0..n {
            let psi = x[(k, i)] * s;
            for j in 0..m {
                a[(k * n + i) * m + j] = C64::new(psi * (row[j] / s), 0.0);
            }
        }
    }
    if a.iter().any(|v| !v.is_finite()) {
        return Err("the modal fit's shapes are not finite".into());
    }
    Ok(Fit { f, z, a })
}

/// Each mode's largest share of the identification within three half-power
/// bandwidths of itself.
fn shares(b: &Band, fit: &Fit) -> Vec<f64> {
    let e = b.n * b.m;
    let nh: Vec<f64> = (0..b.g.len())
        .map(|k| {
            b.h[k * e..(k + 1) * e]
                .iter()
                .map(|v| v.norm_sqr())
                .sum::<f64>()
                .sqrt()
        })
        .collect();
    (0..fit.f.len())
        .map(|k| {
            let (fk, zk) = (fit.f[k], fit.z[k]);
            let na = fit.a[k * e..(k + 1) * e]
                .iter()
                .map(|v| v.norm_sqr())
                .sum::<f64>()
                .sqrt();
            let mut best = 0.0f64;
            for (li, &g) in b.g.iter().enumerate() {
                if (g - fk).abs() <= 3.0 * 2.0 * zk * fk {
                    let q = q_mode(g, fk, zk).norm();
                    best = best.max(q * na / nh[li].max(1e-300));
                }
            }
            best
        })
        .collect()
}

/// The modal fit of `id` over `band` with the known pure delay `tau_s` taken
/// out first and put back on the residues: the modes kept (see the module
/// notes), residues complex.
pub fn fit_known_delay(
    id: &Identification,
    band: (f64, f64),
    tau_s: f64,
) -> Result<ModalForm, String> {
    if !tau_s.is_finite() {
        return Err("the modal fit needs a finite known delay".into());
    }
    if !(band.0 > 0.0 && band.1 > band.0) {
        return Err("the modal fit needs a band above zero".into());
    }
    let b = Band::of(id, band, tau_s);
    let (n, m) = (b.n, b.m);
    let e = n * m;
    let (mut lam, mut part) = poles_over_ranges(&b, band);
    let mut fit = shapes(&b, &lam, &part)?;
    for _ in 0..3 {
        let c = shares(&b, &fit);
        if c.iter().all(|&x| x >= MIN_SHARE) {
            break;
        }
        let keep: Vec<usize> = (0..c.len()).filter(|&k| c[k] >= MIN_SHARE).collect();
        lam = keep.iter().map(|&k| lam[k]).collect();
        part = keep
            .iter()
            .flat_map(|&k| part[k * m..(k + 1) * m].to_vec())
            .collect();
        fit = shapes(&b, &lam, &part)?;
    }
    let mut out = ModalForm {
        f_hz: Vec::new(),
        zeta: Vec::new(),
        residues: Vec::new(),
    };
    for k in 0..fit.f.len() {
        let (f, z) = (fit.f[k], fit.z[k]);
        if f >= band.0 * 0.9 && f <= band.1 * 1.1 && z > 1e-4 && z < 0.2 {
            let turn = C64::from_polar(1.0, -TAU * f * tau_s);
            out.f_hz.push(f);
            out.zeta.push(z);
            out.residues
                .extend(fit.a[k * e..(k + 1) * e].iter().map(|v| v * turn));
        }
    }
    Ok(out)
}
