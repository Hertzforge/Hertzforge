//! A modal model of the plant on the pulse's lines, and its fit from an
//! identified frequency response (`Derivations.md` D9 and D10).
//!
//! The model, per line at angular frequency `w`:
//!
//! `Hh(w) = Ph(w) [ B0 + (f / f_ref)^2 B2 + sum_m q(w; fn_m, z_m) R_m ]`
//!
//! with `Ph = diag(exp(-j w tau_c))` a delay per control,
//! `q = w^2 / (wn^2 - w^2 + 2 j z wn w)`, `wn = 2 pi fn`, complex `n_c x n_d`
//! residue matrices `R_m`, and a complex residual term (`B0`, and optionally
//! `B2`, which stands for modes above the band). The modes share their poles
//! across every element. The model's parameters that move with level are
//! `theta = (fn_1, z_1, fn_2, z_2, ...)`.
//!
//! The fit ([`fit_modal`]) finds the common poles by vector fitting (pole
//! relocation with a relaxed weight function, the fast form that compresses
//! each element's columns by QR), a delay per control by a one-dimensional
//! search of the residue fit's residual, and the residues and residual terms
//! by linear least squares with the poles held. It is deterministic.

use crate::check::eigenvalues;
use crate::lsq::{back_substitute, lstsq, orthonormal_basis};
use crate::{MatrixStack, C64};
use std::f64::consts::TAU;

/// One mode of a [`ModalModel`].
#[derive(Clone, Debug, PartialEq)]
pub struct ModalTerm {
    /// Natural frequency, Hz.
    pub fn_hz: f64,
    /// Damping ratio.
    pub zeta: f64,
    /// Residue matrix, `n_c x n_d` row-major, complex.
    pub residue: Vec<C64>,
}

/// The modal model of a plant (module documentation).
#[derive(Clone, Debug, PartialEq)]
pub struct ModalModel {
    /// Number of controls.
    pub n_c: usize,
    /// Number of drives.
    pub n_d: usize,
    /// A delay per control, seconds.
    pub delays_s: Vec<f64>,
    /// Constant residual term, `n_c x n_d` row-major.
    pub b0: Vec<C64>,
    /// Residual term on `(f / f_ref)^2`, `n_c x n_d` row-major.
    pub b2: Vec<C64>,
    /// Reference frequency of `b2`, Hz.
    pub f_ref_hz: f64,
    /// The modes.
    pub modes: Vec<ModalTerm>,
}

/// `q(w) = w^2 / (wn^2 - w^2 + 2 j z wn w)`.
pub fn modal_q(w: f64, wn: f64, z: f64) -> C64 {
    C64::new(w * w, 0.0) / C64::new(wn * wn - w * w, 2.0 * z * wn * w)
}

/// `q` and its derivatives with respect to `fn` (Hz) and `z`.
pub fn modal_q_derivs(w: f64, fn_hz: f64, z: f64) -> (C64, C64, C64) {
    let wn = TAU * fn_hz;
    let den = C64::new(wn * wn - w * w, 2.0 * z * wn * w);
    let q = C64::new(w * w, 0.0) / den;
    let dq_dwn = -q * C64::new(2.0 * wn, 2.0 * z * w) / den;
    let dq_dz = -q * C64::new(0.0, 2.0 * wn * w) / den;
    (q, dq_dwn * TAU, dq_dz)
}

impl ModalModel {
    /// `theta = (fn_1, z_1, fn_2, z_2, ...)`.
    pub fn theta(&self) -> Vec<f64> {
        self.modes.iter().flat_map(|m| [m.fn_hz, m.zeta]).collect()
    }

    /// The same model with the modes' frequencies and damping from `theta`.
    pub fn with_theta(&self, theta: &[f64]) -> ModalModel {
        let mut out = self.clone();
        for (m, t) in out.modes.iter_mut().zip(theta.chunks(2)) {
            m.fn_hz = t[0];
            m.zeta = t[1];
        }
        out
    }

    /// The model at angular frequency `w` into `out` (`n_c x n_d`, row-major).
    pub fn eval(&self, w: f64, out: &mut [C64]) {
        let x = w / (TAU * self.f_ref_hz);
        let x2 = x * x;
        for (o, (a, b)) in out.iter_mut().zip(self.b0.iter().zip(&self.b2)) {
            *o = *a + *b * x2;
        }
        for m in &self.modes {
            let q = modal_q(w, TAU * m.fn_hz, m.zeta);
            for (o, r) in out.iter_mut().zip(&m.residue) {
                *o += q * *r;
            }
        }
        for r in 0..self.n_c {
            let ph = C64::from_polar(1.0, -w * self.delays_s[r]);
            for c in 0..self.n_d {
                out[r * self.n_d + c] *= ph;
            }
        }
    }

    /// The model on every line of a record of `record_len` samples at `fs_hz`,
    /// real at the Nyquist line (a real record carries only a real value
    /// there).
    pub fn frf(&self, fs_hz: f64, record_len: usize) -> MatrixStack {
        let w = crate::spectrum::omegas(fs_hz, record_len);
        let mut h = MatrixStack::zeros(w.len(), self.n_c, self.n_d);
        for (f, &wf) in w.iter().enumerate() {
            self.eval(wf, h.line_mut(f));
        }
        let last = w.len() - 1;
        for v in h.line_mut(last) {
            *v = C64::new(v.re, 0.0);
        }
        h
    }

    /// `base` with the lines flagged in `band` replaced by the model (the
    /// model is trusted only where it was fitted).
    pub fn frf_in_band(&self, base: &MatrixStack, band: &[bool], fs_hz: f64) -> MatrixStack {
        let mut h = base.clone();
        let lines = h.lines();
        let record_len = 2 * (lines - 1);
        let df = fs_hz / record_len as f64;
        for (f, &on) in band.iter().enumerate() {
            if on {
                self.eval(TAU * f as f64 * df, h.line_mut(f));
                if f == lines - 1 {
                    for v in h.line_mut(f) {
                        *v = C64::new(v.re, 0.0);
                    }
                }
            }
        }
        h
    }

    /// The Frobenius norm of each mode's residue.
    pub fn residue_norms(&self) -> Vec<f64> {
        self.modes
            .iter()
            .map(|m| m.residue.iter().map(|v| v.norm_sqr()).sum::<f64>().sqrt())
            .collect()
    }
}

/// Options of [`fit_modal`].
#[derive(Clone, Debug, PartialEq)]
pub struct FitOptions {
    /// Lowest line fitted, Hz.
    pub band_lo_hz: f64,
    /// Highest line fitted, Hz.
    pub band_hi_hz: f64,
    /// Number of modes (complex pole pairs) to fit.
    pub n_modes: usize,
    /// Pole relocation iterations in each pass.
    pub pole_iterations: usize,
    /// Passes of pole relocation followed by the delay search.
    pub passes: usize,
    /// Each control's delay is searched in `+/- delay_max_s`; zero fits none.
    pub delay_max_s: f64,
    /// Whether the residual term on `(f / f_ref)^2` is fitted.
    pub residual_mass: bool,
    /// Gauss-Newton refinements of the poles and delays in the model's own
    /// form after the pole relocation (each kept only if it lowers the
    /// residual).
    pub refine_iterations: usize,
}

impl Default for FitOptions {
    fn default() -> Self {
        FitOptions {
            band_lo_hz: 80.0,
            band_hi_hz: 2400.0,
            n_modes: 4,
            pole_iterations: 4,
            passes: 2,
            delay_max_s: 1.0e-3,
            residual_mass: true,
            refine_iterations: 20,
        }
    }
}

/// The result of [`fit_modal`].
#[derive(Clone, Debug, PartialEq)]
pub struct ModalFit {
    /// The fitted model (modes sorted by frequency).
    pub model: ModalModel,
    /// `||Hh - H|| / ||H||` over the band's lines and every element.
    pub residual_rel: f64,
    /// Real poles the relocation produced, not kept as modes.
    pub real_poles_dropped: usize,
    /// The lines the fit used.
    pub band: Vec<bool>,
}

#[derive(Clone, Copy, Debug)]
enum Pole {
    Real(f64),
    Pair(C64),
}

fn basis_count(poles: &[Pole]) -> usize {
    poles
        .iter()
        .map(|p| match p {
            Pole::Real(_) => 1,
            Pole::Pair(_) => 2,
        })
        .sum()
}

/// The real-coefficient basis of the poles at `s = j w`.
fn basis_at(poles: &[Pole], w: f64, out: &mut Vec<C64>) {
    out.clear();
    let s = C64::new(0.0, w);
    let j = C64::new(0.0, 1.0);
    for p in poles {
        match *p {
            Pole::Real(a) => out.push(C64::new(1.0, 0.0) / (s - a)),
            Pole::Pair(a) => {
                let u = C64::new(1.0, 0.0) / (s - a);
                let v = C64::new(1.0, 0.0) / (s - a.conj());
                out.push(u + v);
                out.push(j * u - j * v);
            }
        }
    }
}

/// One pole relocation of vector fitting (relaxed). `data[k][i]` is element
/// `k` at `w[i]`.
fn relocate(poles: &[Pole], w: &[f64], data: &[Vec<C64>]) -> Result<Vec<Pole>, String> {
    let n = basis_count(poles);
    let l = w.len();
    let rows = 2 * l;
    let mut phis: Vec<Vec<C64>> = Vec::with_capacity(l);
    let mut buf = Vec::new();
    for &wi in w {
        basis_at(poles, wi, &mut buf);
        phis.push(buf.clone());
    }
    // The columns [Phi, 1] are shared by every element: project them out once
    // (an orthonormal basis Q1), then each element's weight-function block is
    // the triangular factor of its projected columns, from their Gram matrix.
    let n1 = n + 1;
    let mut a1 = vec![0.0; rows * n1];
    for i in 0..l {
        for c in 0..n {
            a1[i * n1 + c] = phis[i][c].re;
            a1[(l + i) * n1 + c] = phis[i][c].im;
        }
        a1[i * n1 + n] = 1.0;
    }
    let (q1, _) = orthonormal_basis(&a1, rows, n1);
    let mut global: Vec<f64> = Vec::new();
    let mut nrows = 0usize;
    let mut scale2 = 0.0;
    let mut a2 = vec![0.0; rows * n1];
    let mut proj = vec![0.0; n1 * n1];
    let mut gram = vec![0.0; n1 * n1];
    for fk in data {
        scale2 += fk.iter().map(|v| v.norm_sqr()).sum::<f64>();
        for i in 0..l {
            let phi = &phis[i];
            for c in 0..n {
                let t = -fk[i] * phi[c];
                a2[i * n1 + c] = t.re;
                a2[(l + i) * n1 + c] = t.im;
            }
            a2[i * n1 + n] = -fk[i].re;
            a2[(l + i) * n1 + n] = -fk[i].im;
        }
        proj.iter_mut().for_each(|v| *v = 0.0);
        gram.iter_mut().for_each(|v| *v = 0.0);
        for i in 0..rows {
            let qa = &q1[i * n1..(i + 1) * n1];
            let aa = &a2[i * n1..(i + 1) * n1];
            for (a, &qv) in qa.iter().enumerate() {
                if qv != 0.0 {
                    for (b, &av) in aa.iter().enumerate() {
                        proj[a * n1 + b] += qv * av;
                    }
                }
            }
            for (a, &x) in aa.iter().enumerate() {
                for b in a..n1 {
                    gram[a * n1 + b] += x * aa[b];
                }
            }
        }
        // Gram of the projected columns: A2^T A2 - (Q1^T A2)^T (Q1^T A2).
        for a in 0..n1 {
            for b in a..n1 {
                let mut v = gram[a * n1 + b];
                for k in 0..n1 {
                    v -= proj[k * n1 + a] * proj[k * n1 + b];
                }
                gram[a * n1 + b] = v;
                gram[b * n1 + a] = v;
            }
        }
        // Its upper-triangular factor (Cholesky, with a tiny floor).
        let mut r = vec![0.0; n1 * n1];
        let tr = (0..n1)
            .map(|i| gram[i * n1 + i])
            .sum::<f64>()
            .max(f64::MIN_POSITIVE);
        for i in 0..n1 {
            for j in i..n1 {
                let mut v = gram[i * n1 + j];
                for k in 0..i {
                    v -= r[k * n1 + i] * r[k * n1 + j];
                }
                if i == j {
                    r[i * n1 + i] = v.max(1e-14 * tr).sqrt();
                } else {
                    r[i * n1 + j] = v / r[i * n1 + i];
                }
            }
        }
        global.extend_from_slice(&r);
        nrows += n1;
    }
    // The relaxation's normalization: Re(sum over the band of sigma) = l.
    let scale = scale2.sqrt() / l as f64;
    let mut con = vec![0.0; n + 1];
    for phi in &phis {
        for c in 0..n {
            con[c] += scale * phi[c].re;
        }
    }
    con[n] = scale * l as f64;
    let mut rhs = vec![0.0; nrows];
    global.extend_from_slice(&con);
    rhs.push(scale * l as f64);
    let (x, _) = lstsq(&global, nrows + 1, n + 1, &rhs)?;
    let (ct, dt) = if x[n].abs() > 1e-8 {
        (x[..n].to_vec(), x[n])
    } else {
        // Non-relaxed: the weight's constant fixed at one.
        let mut a = Vec::with_capacity(nrows * n);
        let mut b = Vec::with_capacity(nrows);
        for r in 0..nrows {
            a.extend_from_slice(&global[r * (n + 1)..r * (n + 1) + n]);
            b.push(-global[r * (n + 1) + n]);
        }
        (lstsq(&a, nrows, n, &b)?.0, 1.0)
    };
    // The zeros of sigma: eig(A - b ct / dt).
    let mut a = vec![0.0; n * n];
    let mut bv = vec![0.0; n];
    let mut k = 0;
    for p in poles {
        match *p {
            Pole::Real(r) => {
                a[k * n + k] = r;
                bv[k] = 1.0;
                k += 1;
            }
            Pole::Pair(c) => {
                a[k * n + k] = c.re;
                a[k * n + k + 1] = c.im;
                a[(k + 1) * n + k] = -c.im;
                a[(k + 1) * n + k + 1] = c.re;
                bv[k] = 2.0;
                k += 2;
            }
        }
    }
    let z: Vec<C64> = (0..n * n)
        .map(|idx| {
            let (i, j) = (idx / n, idx % n);
            C64::new(a[idx] - bv[i] * ct[j] / dt, 0.0)
        })
        .collect();
    let ev = eigenvalues(&z, n)?;
    let mut out = Vec::new();
    for e in ev {
        let e = if e.re > 0.0 { C64::new(-e.re, e.im) } else { e };
        if e.im.abs() <= 1e-9 * e.norm().max(1.0) {
            out.push(Pole::Real(e.re.min(-1e-6)));
        } else if e.im > 0.0 {
            out.push(Pole::Pair(e));
        }
    }
    if out.is_empty()
        || !out.iter().all(|p| match p {
            Pole::Real(r) => r.is_finite(),
            Pole::Pair(c) => c.re.is_finite() && c.im.is_finite(),
        })
    {
        return Err("the pole relocation failed".into());
    }
    Ok(out)
}

/// The residue fit with the poles held: an orthonormal basis of the real
/// stacked columns `[1, (f/f_ref)^2, q_1, .., q_M]` (each complex coefficient
/// as two real ones), shared by every element.
struct ResidueBasis {
    q: Vec<f64>,
    r: Vec<f64>,
    p: usize,
    rows: usize,
}

impl ResidueBasis {
    fn new(modes: &[(f64, f64)], w: &[f64], f_ref_hz: f64, residual_mass: bool) -> Self {
        let l = w.len();
        let ncx = modes.len() + 1 + usize::from(residual_mass);
        let p = 2 * ncx;
        let rows = 2 * l;
        let mut a = vec![0.0; rows * p];
        for (i, &wi) in w.iter().enumerate() {
            let x = wi / (TAU * f_ref_hz);
            let mut fns: Vec<C64> = vec![C64::new(1.0, 0.0)];
            if residual_mass {
                fns.push(C64::new(x * x, 0.0));
            }
            for &(fnh, z) in modes {
                fns.push(modal_q(wi, TAU * fnh, z));
            }
            for (c, phi) in fns.iter().enumerate() {
                // coefficient a + j b: Re = phi.re a - phi.im b, Im = phi.im a + phi.re b
                a[i * p + 2 * c] = phi.re;
                a[i * p + 2 * c + 1] = -phi.im;
                a[(l + i) * p + 2 * c] = phi.im;
                a[(l + i) * p + 2 * c + 1] = phi.re;
            }
        }
        let (q, r) = orthonormal_basis(&a, rows, p);
        ResidueBasis { q, r, p, rows }
    }

    /// Residual sum of squares and the projection `Q^T b` of the stacked data.
    fn project(&self, b: &[f64]) -> (f64, Vec<f64>) {
        let p = self.p;
        let mut qtb = vec![0.0; p];
        for (i, &bi) in b.iter().enumerate().take(self.rows) {
            if bi == 0.0 {
                continue;
            }
            let row = &self.q[i * p..(i + 1) * p];
            for (t, qv) in qtb.iter_mut().zip(row) {
                *t += qv * bi;
            }
        }
        let bb: f64 = b.iter().map(|v| v * v).sum();
        let pp: f64 = qtb.iter().map(|v| v * v).sum();
        ((bb - pp).max(0.0), qtb)
    }

    /// Complex coefficients from `Q^T b`.
    fn coefficients(&self, qtb: &[f64]) -> Vec<C64> {
        let x = back_substitute(&self.r, self.p, self.p, qtb, 1e-12);
        x.chunks(2).map(|c| C64::new(c[0], c[1])).collect()
    }
}

fn stacked(h: &MatrixStack, lines: &[usize], w: &[f64], el: usize, tau: f64, out: &mut [f64]) {
    let l = lines.len();
    for (i, (&f, &wi)) in lines.iter().zip(w).enumerate() {
        let v = h.line(f)[el] * C64::from_polar(1.0, wi * tau);
        out[i] = v.re;
        out[l + i] = v.im;
    }
}

/// Fits the modal model to the identified `h` (module documentation).
pub fn fit_modal(h: &MatrixStack, fs_hz: f64, opts: &FitOptions) -> Result<ModalFit, String> {
    let (lines, n_c, n_d) = h.dims();
    if lines < 3 {
        return Err("the identification has too few lines".into());
    }
    if opts.n_modes == 0 {
        return Err("the fit needs at least one mode".into());
    }
    if !(opts.band_lo_hz >= 0.0 && opts.band_hi_hz > opts.band_lo_hz) {
        return Err("the fit band must be ascending".into());
    }
    let record_len = 2 * (lines - 1);
    let df = fs_hz / record_len as f64;
    let band: Vec<bool> = (0..lines)
        .map(|k| {
            let f = k as f64 * df;
            f >= opts.band_lo_hz && f <= opts.band_hi_hz && k > 0
        })
        .collect();
    let idx: Vec<usize> = (0..lines).filter(|&k| band[k]).collect();
    if idx.len() < 4 * opts.n_modes + 4 {
        return Err("the band holds too few lines for the model order".into());
    }
    let w: Vec<f64> = idx.iter().map(|&k| TAU * k as f64 * df).collect();
    let l = idx.len();
    let n_el = n_c * n_d;
    let f_ref = opts.band_hi_hz;

    // Starting poles: pairs spread in log frequency over the band, damping 0.01.
    let (wlo, whi) = (TAU * opts.band_lo_hz.max(df), TAU * opts.band_hi_hz);
    let mut poles: Vec<Pole> = (0..opts.n_modes)
        .map(|i| {
            let t = (i as f64 + 0.5) / opts.n_modes as f64;
            let b = wlo * (whi / wlo).powf(t);
            Pole::Pair(C64::new(-b / 100.0, b))
        })
        .collect();
    let mut delays = vec![0.0; n_c];
    let mut buf = vec![0.0; 2 * l];
    let passes = opts.passes.max(1);
    for _pass in 0..passes {
        let data: Vec<Vec<C64>> = (0..n_el)
            .map(|el| {
                let tau = delays[el / n_d];
                idx.iter()
                    .zip(&w)
                    .map(|(&f, &wi)| h.line(f)[el] * C64::from_polar(1.0, wi * tau))
                    .collect()
            })
            .collect();
        for _ in 0..opts.pole_iterations.max(1) {
            poles = relocate(&poles, &w, &data)?;
        }
        if opts.delay_max_s > 0.0 {
            let modes = pairs_of(&poles);
            let basis = ResidueBasis::new(&modes, &w, f_ref, opts.residual_mass);
            for (c, tau_c) in delays.iter_mut().enumerate() {
                let cost = |tau: f64, buf: &mut [f64]| -> f64 {
                    (0..n_d)
                        .map(|j| {
                            stacked(h, &idx, &w, c * n_d + j, tau, buf);
                            basis.project(buf).0
                        })
                        .sum()
                };
                let tm = opts.delay_max_s;
                let steps = 80usize;
                let step = 2.0 * tm / steps as f64;
                let mut best = (f64::INFINITY, 0.0);
                for i in 0..=steps {
                    let t = -tm + i as f64 * step;
                    let v = cost(t, &mut buf);
                    if v < best.0 {
                        best = (v, t);
                    }
                }
                // Golden-section refinement inside one grid step either side.
                let g = 0.5 * (5f64.sqrt() - 1.0);
                let (mut a, mut b) = (best.1 - step, best.1 + step);
                let mut x1 = b - g * (b - a);
                let mut x2 = a + g * (b - a);
                let mut f1 = cost(x1, &mut buf);
                let mut f2 = cost(x2, &mut buf);
                for _ in 0..40 {
                    if f1 < f2 {
                        b = x2;
                        x2 = x1;
                        f2 = f1;
                        x1 = b - g * (b - a);
                        f1 = cost(x1, &mut buf);
                    } else {
                        a = x1;
                        x1 = x2;
                        f1 = f2;
                        x2 = a + g * (b - a);
                        f2 = cost(x2, &mut buf);
                    }
                }
                *tau_c = 0.5 * (a + b);
            }
        }
    }

    let real_dropped = poles.iter().filter(|p| matches!(p, Pole::Real(_))).count();
    let mut modes = pairs_of(&poles);
    modes.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut model = residues_for(h, &idx, &w, &modes, &delays, f_ref, opts.residual_mass);
    for _ in 0..opts.refine_iterations {
        let before = relative_residual(&model, h, &band, fs_hz);
        let (theta, taus) = refine_step(&model, h, &idx, &w, opts.residual_mass);
        let trial = residues_for(
            h,
            &idx,
            &w,
            &theta.chunks(2).map(|t| (t[0], t[1])).collect::<Vec<_>>(),
            &taus,
            f_ref,
            opts.residual_mass,
        );
        let after = relative_residual(&trial, h, &band, fs_hz);
        if after.is_nan() || after >= before {
            break;
        }
        model = trial;
        if before - after <= 1e-9 * before {
            break;
        }
    }
    let residual_rel = relative_residual(&model, h, &band, fs_hz);
    Ok(ModalFit {
        model,
        residual_rel,
        real_poles_dropped: real_dropped,
        band,
    })
}

/// Residues and residual terms by linear least squares with the poles and
/// delays held.
fn residues_for(
    h: &MatrixStack,
    idx: &[usize],
    w: &[f64],
    modes: &[(f64, f64)],
    delays: &[f64],
    f_ref: f64,
    residual_mass: bool,
) -> ModalModel {
    let (_, n_c, n_d) = h.dims();
    let n_el = n_c * n_d;
    let basis = ResidueBasis::new(modes, w, f_ref, residual_mass);
    let mut buf = vec![0.0; 2 * idx.len()];
    let mut b0 = vec![C64::new(0.0, 0.0); n_el];
    let mut b2 = vec![C64::new(0.0, 0.0); n_el];
    let mut res: Vec<Vec<C64>> = vec![vec![C64::new(0.0, 0.0); n_el]; modes.len()];
    for el in 0..n_el {
        stacked(h, idx, w, el, delays[el / n_d], &mut buf);
        let (_, qtb) = basis.project(&buf);
        let coef = basis.coefficients(&qtb);
        b0[el] = coef[0];
        let off = if residual_mass {
            b2[el] = coef[1];
            2
        } else {
            1
        };
        for (m, r) in res.iter_mut().enumerate() {
            r[el] = coef[off + m];
        }
    }
    ModalModel {
        n_c,
        n_d,
        delays_s: delays.to_vec(),
        b0,
        b2,
        f_ref_hz: f_ref,
        modes: modes
            .iter()
            .zip(res)
            .map(|(&(fn_hz, zeta), residue)| ModalTerm {
                fn_hz,
                zeta,
                residue,
            })
            .collect(),
    }
}

/// One Gauss-Newton step on every mode's frequency and damping and every
/// control's delay, with the residues eliminated (variable projection with
/// Kaufman's Jacobian: each column is the model's derivative with its
/// component in the span of the linear basis removed, element by element;
/// `Derivations.md` D10, step 3).
fn refine_step(
    model: &ModalModel,
    h: &MatrixStack,
    idx: &[usize],
    w: &[f64],
    residual_mass: bool,
) -> (Vec<f64>, Vec<f64>) {
    let (n_c, n_d) = (model.n_c, model.n_d);
    let nm = model.modes.len();
    let np = 2 * nm + n_c;
    let l = idx.len();
    let modes: Vec<(f64, f64)> = model.modes.iter().map(|m| (m.fn_hz, m.zeta)).collect();
    let basis = ResidueBasis::new(&modes, w, model.f_ref_hz, residual_mass);
    let derivs: Vec<Vec<(C64, C64)>> = w
        .iter()
        .map(|&wi| {
            model
                .modes
                .iter()
                .map(|m| {
                    let (_, a, b) = modal_q_derivs(wi, m.fn_hz, m.zeta);
                    (a, b)
                })
                .collect()
        })
        .collect();
    let mut nrm = vec![0.0; np * np];
    let mut g = vec![0.0; np];
    let mut pred = vec![C64::new(0.0, 0.0); n_c * n_d];
    let mut preds: Vec<Vec<C64>> = vec![vec![C64::new(0.0, 0.0); l]; n_c * n_d];
    for (i, &wi) in w.iter().enumerate() {
        model.eval(wi, &mut pred);
        for (el, p) in pred.iter().enumerate() {
            preds[el][i] = *p;
        }
    }
    let mut res = vec![0.0; 2 * l];
    let mut cols: Vec<Vec<f64>> = vec![vec![0.0; 2 * l]; 2 * nm + 1];
    for r in 0..n_c {
        for c in 0..n_d {
            let el = r * n_d + c;
            for (i, (&f, &wi)) in idx.iter().zip(w).enumerate() {
                // Compensated domain: divide by the row's delay phasor.
                let back = C64::from_polar(1.0, wi * model.delays_s[r]);
                let rv = (h.line(f)[el] - preds[el][i]) * back;
                res[i] = rv.re;
                res[l + i] = rv.im;
                for m in 0..nm {
                    let rr = model.modes[m].residue[el];
                    let a = derivs[i][m].0 * rr;
                    let b = derivs[i][m].1 * rr;
                    cols[2 * m][i] = a.re;
                    cols[2 * m][l + i] = a.im;
                    cols[2 * m + 1][i] = b.re;
                    cols[2 * m + 1][l + i] = b.im;
                }
                let d = C64::new(0.0, -wi) * preds[el][i] * back;
                cols[2 * nm][i] = d.re;
                cols[2 * nm][l + i] = d.im;
            }
            for col in cols.iter_mut() {
                let (_, qtc) = basis.project(col);
                let p = basis.p;
                for (i, cv) in col.iter_mut().enumerate().take(2 * l) {
                    let row = &basis.q[i * p..(i + 1) * p];
                    let s: f64 = row.iter().zip(&qtc).map(|(a, b)| a * b).sum();
                    *cv -= s;
                }
            }
            let gi = |k: usize| if k < 2 * nm { k } else { 2 * nm + r };
            for a in 0..=2 * nm {
                let ia = gi(a);
                g[ia] += cols[a].iter().zip(&res).map(|(x, y)| x * y).sum::<f64>();
                for b in 0..=2 * nm {
                    let ib = gi(b);
                    nrm[ia * np + ib] += cols[a]
                        .iter()
                        .zip(&cols[b])
                        .map(|(x, y)| x * y)
                        .sum::<f64>();
                }
            }
        }
    }
    let sc: Vec<f64> = (0..np)
        .map(|i| nrm[i * np + i].sqrt().max(1e-300))
        .collect();
    for i in 0..np {
        g[i] /= sc[i];
        for j in 0..np {
            nrm[i * np + j] /= sc[i] * sc[j];
        }
    }
    let step = crate::lsq::spd_solve(&nrm, np, &g, 1e-12).unwrap_or_else(|_| vec![0.0; np]);
    let mut theta = model.theta();
    for i in 0..2 * nm {
        theta[i] += step[i] / sc[i];
    }
    for t in theta.chunks_mut(2) {
        t[1] = t[1].max(1e-6);
    }
    let taus: Vec<f64> = (0..n_c)
        .map(|r| model.delays_s[r] + step[2 * nm + r] / sc[2 * nm + r])
        .collect();
    (theta, taus)
}

fn pairs_of(poles: &[Pole]) -> Vec<(f64, f64)> {
    poles
        .iter()
        .filter_map(|p| match *p {
            Pole::Pair(c) => {
                let wn = c.norm();
                Some((wn / TAU, (-c.re / wn).max(1e-6)))
            }
            Pole::Real(_) => None,
        })
        .collect()
}

/// `||Hh - H|| / ||H||` of a model against `h` over the flagged lines.
pub fn relative_residual(model: &ModalModel, h: &MatrixStack, band: &[bool], fs_hz: f64) -> f64 {
    let (lines, n_c, n_d) = h.dims();
    let record_len = 2 * (lines - 1);
    let df = fs_hz / record_len as f64;
    let mut buf = vec![C64::new(0.0, 0.0); n_c * n_d];
    let (mut num, mut den) = (0.0, 0.0);
    for (f, &on) in band.iter().enumerate() {
        if !on {
            continue;
        }
        model.eval(TAU * f as f64 * df, &mut buf);
        for (a, b) in buf.iter().zip(h.line(f)) {
            num += (a - b).norm_sqr();
            den += b.norm_sqr();
        }
    }
    if den > 0.0 {
        (num / den).sqrt()
    } else {
        num.sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_mode_model() -> ModalModel {
        ModalModel {
            n_c: 2,
            n_d: 2,
            delays_s: vec![0.0, 0.2e-3],
            b0: vec![
                C64::new(1.0, 0.0),
                C64::new(0.2, 0.0),
                C64::new(0.3, 0.0),
                C64::new(0.9, 0.0),
            ],
            b2: vec![C64::new(0.0, 0.0); 4],
            f_ref_hz: 2400.0,
            modes: vec![
                ModalTerm {
                    fn_hz: 500.0,
                    zeta: 0.02,
                    residue: vec![
                        C64::new(1.0, 0.1),
                        C64::new(0.5, 0.0),
                        C64::new(-0.4, 0.05),
                        C64::new(0.3, 0.0),
                    ],
                },
                ModalTerm {
                    fn_hz: 1300.0,
                    zeta: 0.015,
                    residue: vec![
                        C64::new(0.3, 0.0),
                        C64::new(-0.6, -0.1),
                        C64::new(0.7, 0.0),
                        C64::new(0.5, 0.0),
                    ],
                },
            ],
        }
    }

    #[test]
    fn mimo_shock_modal_fit_recovers_an_exact_model() {
        let truth = two_mode_model();
        let fs = 8192.0;
        let h = truth.frf(fs, 8192);
        let fit = fit_modal(
            &h,
            fs,
            &FitOptions {
                n_modes: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(fit.residual_rel < 1e-6, "residual {}", fit.residual_rel);
        for (a, b) in fit.model.modes.iter().zip(&truth.modes) {
            assert!(
                (a.fn_hz / b.fn_hz - 1.0).abs() < 1e-6,
                "{} {}",
                a.fn_hz,
                b.fn_hz
            );
            assert!(
                (a.zeta / b.zeta - 1.0).abs() < 1e-4,
                "{} {}",
                a.zeta,
                b.zeta
            );
        }
        assert!((fit.model.delays_s[1] - fit.model.delays_s[0] - 0.2e-3).abs() < 1e-8);
    }

    #[test]
    fn mimo_shock_modal_q_derivatives_match_differences() {
        let (w, f, z) = (TAU * 480.0, 500.0, 0.02);
        let (_, dfn, dz) = modal_q_derivs(w, f, z);
        let h = 1e-6;
        let num_f = (modal_q(w, TAU * (f + h), z) - modal_q(w, TAU * (f - h), z)) / (2.0 * h);
        let num_z = (modal_q(w, TAU * f, z + h * 1e-3) - modal_q(w, TAU * f, z - h * 1e-3))
            / (2.0 * h * 1e-3);
        assert!((num_f - dfn).norm() < 1e-6 * dfn.norm());
        assert!((num_z - dz).norm() < 1e-5 * dz.norm());
    }
}
