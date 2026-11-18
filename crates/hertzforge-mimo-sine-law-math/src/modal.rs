//! The model the carry runs through: the identification's modes kept exact
//! and the rest smoothed.
//!
//! A carried drive is `Z(f) R` to first order, and the regularized inverse of
//! a coupled structure near a light mode amplifies the identification's
//! line-to-line estimation scatter in its weak directions. A correction alone
//! only steers its increments through the model and averages that scatter out;
//! a carry follows it. So the carry reads a model whose remainder, after the
//! modes are taken out, is smoothed (it is smooth in truth), while the modal
//! part stays exact:
//!
//! ```text
//! Hs(f) = sum_r A_r q(f; f_r, zeta_r) + smooth[Hhat0 - sum_r A_r q(f; f_r, zeta_r)]
//! q(f; f_r, zeta_r) = -u^2 / (1 - u^2 + 2 j zeta_r u),  u = f / f_r
//! ```
//!
//! The modes come from the identification alone: candidate peaks of its first
//! two singular values with 4 dB prominence within 8 percent either side,
//! merged when closer than 0.4 percent; damping started from the half-power
//! width of the first singular value; candidates whose windows overlap fitted
//! together, frequency and log damping by Levenberg-Marquardt with
//! finite-difference derivatives, the residues and a constant-plus-slope
//! background solved by linear least squares at each trial (variable
//! projection). The smoothing is a local quadratic over `+- lines`.

use crate::linalg::{reg_pinv, singular_values, solve_real};
use crate::setup::{interp_stack, Identification};
use crate::C64;

const ZERO: C64 = C64::new(0.0, 0.0);

/// The modal response shape `q(f; f_r, zeta_r)` (acceleration per modal force).
pub fn q_mode(f: f64, fr: f64, zr: f64) -> C64 {
    let u = f / fr;
    -C64::new(u * u, 0.0) / C64::new(1.0 - u * u, 2.0 * zr * u)
}

/// The modes found in an identification.
#[derive(Clone, Debug, PartialEq)]
pub struct ModalForm {
    /// Natural frequencies, Hz.
    pub f_hz: Vec<f64>,
    /// Damping ratios.
    pub zeta: Vec<f64>,
    /// Residues at the controls, `modes x controls x drives`.
    pub residues: Vec<C64>,
}

impl ModalForm {
    /// Number of modes.
    pub fn modes(&self) -> usize {
        self.f_hz.len()
    }

    /// The fit described in the module's documentation, over `band` (Hz).
    pub fn fit(id: &Identification, band: (f64, f64)) -> Result<ModalForm, String> {
        fit(id, band, 4.0, 24)
    }
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-300).log10()
}

fn fit(
    id: &Identification,
    band: (f64, f64),
    min_prom_db: f64,
    max_modes: usize,
) -> Result<ModalForm, String> {
    let g = &id.grid_hz;
    let l = g.len();
    let (_, n, m) = id.response.dims();
    let kmin = n.min(m);
    let mut s = vec![0.0; l * kmin];
    for k in 0..l {
        let sv = singular_values(id.response.line(k), n, m)?;
        s[k * kmin..(k + 1) * kmin].copy_from_slice(&sv);
    }
    let mut cands: Vec<(f64, f64)> = Vec::new();
    for i in 0..kmin.min(2) {
        let y: Vec<f64> = (0..l).map(|k| db(s[k * kmin + i])).collect();
        for k in 2..l.saturating_sub(2) {
            if y[k] >= y[k - 1] && y[k] > y[k + 1] && y[k] >= y[k - 2] && y[k] > y[k + 2] {
                let a = g.partition_point(|&x| x < g[k] * 0.92);
                let b = g.partition_point(|&x| x < g[k] * 1.08).min(l - 1);
                let lo1 = y[a..=k].iter().copied().fold(f64::INFINITY, f64::min);
                let lo2 = y[k..=b].iter().copied().fold(f64::INFINITY, f64::min);
                let prom = y[k] - lo1.min(lo2);
                if prom >= min_prom_db && band.0 <= g[k] && g[k] <= band.1 {
                    cands.push((g[k], prom));
                }
            }
        }
    }
    cands.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut picked: Vec<f64> = Vec::new();
    for &(fk, _) in &cands {
        if picked.iter().all(|&p| (fk - p).abs() > 0.004 * fk) {
            picked.push(fk);
        }
    }
    picked.truncate(max_modes);
    picked.sort_by(f64::total_cmp);
    let mut out = ModalForm {
        f_hz: Vec::new(),
        zeta: Vec::new(),
        residues: Vec::new(),
    };
    if picked.is_empty() {
        return Ok(out);
    }
    let s1: Vec<f64> = (0..l).map(|k| s[k * kmin]).collect();
    let z_init: Vec<f64> = picked
        .iter()
        .map(|&fk| {
            let k = argmin_abs(g, fk);
            let half = s1[k] / 2f64.sqrt();
            let mut a = k;
            while a > 0 && s1[a] > half && g[k] - g[a] < 0.1 * fk {
                a -= 1;
            }
            let mut b = k;
            while b < l - 1 && s1[b] > half && g[b] - g[k] < 0.1 * fk {
                b += 1;
            }
            ((g[b] - g[a]) / (2.0 * fk)).clamp(0.003, 0.1)
        })
        .collect();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut cur = vec![0usize];
    for j in 1..picked.len() {
        let last = *cur.last().unwrap_or(&0);
        if picked[j] - picked[last] < 5.0 * (z_init[j] * picked[j] + z_init[last] * picked[last]) {
            cur.push(j);
        } else {
            groups.push(std::mem::replace(&mut cur, vec![j]));
        }
    }
    groups.push(cur);
    for grp in groups {
        let kk = grp.len();
        let fr: Vec<f64> = grp.iter().map(|&j| picked[j]).collect();
        let zr: Vec<f64> = grp.iter().map(|&j| z_init[j]).collect();
        let zmax = zr.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let fmin = fr.iter().copied().fold(f64::INFINITY, f64::min);
        let fmax = fr.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let (lo, hi) = (fmin * (1.0 - 5.0 * zmax), fmax * (1.0 + 5.0 * zmax));
        let mut sel: Vec<usize> = (0..l).filter(|&k| g[k] >= lo && g[k] <= hi).collect();
        if sel.len() < 2 * kk + 4 {
            let mean = fr.iter().sum::<f64>() / kk as f64;
            let c = argmin_abs(g, mean);
            sel = (c.saturating_sub(kk + 3)..(c + kk + 4).min(l)).collect();
        }
        let window = Window::new(id, &sel);
        let (fr, zr, a) = window.fit_group(fr, zr)?;
        for r in 0..kk {
            if band.0 * 0.9 <= fr[r] && fr[r] <= band.1 * 1.1 && 1e-4 < zr[r] && zr[r] < 0.2 {
                out.f_hz.push(fr[r]);
                out.zeta.push(zr[r]);
                out.residues
                    .extend_from_slice(&a[r * n * m..(r + 1) * n * m]);
            }
        }
    }
    Ok(out)
}

fn argmin_abs(g: &[f64], f: f64) -> usize {
    let mut best = 0;
    for k in 1..g.len() {
        if (g[k] - f).abs() < (g[best] - f).abs() {
            best = k;
        }
    }
    best
}

/// The lines a group of modes is fitted over.
struct Window {
    gg: Vec<f64>,
    /// `lines x (controls * drives)`.
    y: Vec<C64>,
    e: usize,
}

impl Window {
    fn new(id: &Identification, sel: &[usize]) -> Window {
        let (_, n, m) = id.response.dims();
        let e = n * m;
        let mut y = Vec::with_capacity(sel.len() * e);
        for &k in sel {
            y.extend_from_slice(id.response.line(k));
        }
        Window {
            gg: sel.iter().map(|&k| id.grid_hz[k]).collect(),
            y,
            e,
        }
    }

    /// For fixed poles: least squares of the residues and a constant-plus-slope
    /// background over every element at once. `(cost, residues)`.
    fn lsq(&self, fr: &[f64], zr: &[f64]) -> Result<(f64, Vec<C64>), String> {
        let l = self.gg.len();
        let k = fr.len();
        let cols = k + 2;
        let fc = self.gg.iter().sum::<f64>() / l as f64;
        let mut b = vec![ZERO; l * cols];
        for (i, &f) in self.gg.iter().enumerate() {
            for r in 0..k {
                b[i * cols + r] = q_mode(f, fr[r], zr[r]);
            }
            b[i * cols + k] = C64::new(1.0, 0.0);
            b[i * cols + k + 1] = C64::new((f - fc) / fc, 0.0);
        }
        let (bp, _) = reg_pinv(&b, l, cols, 0.0)?; // cols x l
        let e = self.e;
        let mut x = vec![ZERO; cols * e];
        for c in 0..cols {
            for i in 0..l {
                let w = bp[c * l + i];
                for el in 0..e {
                    x[c * e + el] += w * self.y[i * e + el];
                }
            }
        }
        let mut cost = 0.0;
        for i in 0..l {
            for el in 0..e {
                let mut p = ZERO;
                for c in 0..cols {
                    p += b[i * cols + c] * x[c * e + el];
                }
                cost += (self.y[i * e + el] - p).norm_sqr();
            }
        }
        x.truncate(k * e);
        Ok((cost, x))
    }

    fn cost(&self, th: &[f64], k: usize) -> Result<f64, String> {
        let zr: Vec<f64> = th[k..].iter().map(|v| v.exp()).collect();
        Ok(self.lsq(&th[..k], &zr)?.0)
    }

    /// Levenberg-Marquardt over `(f_r, ln zeta_r)`.
    fn fit_group(
        &self,
        fr: Vec<f64>,
        zr: Vec<f64>,
    ) -> Result<(Vec<f64>, Vec<f64>, Vec<C64>), String> {
        let k = fr.len();
        let np = 2 * k;
        let mut th: Vec<f64> = fr
            .iter()
            .copied()
            .chain(zr.iter().map(|z| z.ln()))
            .collect();
        let mut c0 = self.cost(&th, k)?;
        let mut lam = 1e-2;
        let shift = |t: &[f64], d: &[(usize, f64)]| {
            let mut v = t.to_vec();
            for &(i, h) in d {
                v[i] += h;
            }
            v
        };
        for _ in 0..40 {
            let h: Vec<f64> = (0..np)
                .map(|i| if i < k { 1e-5 * th[i] } else { 1e-4 })
                .collect();
            let mut gr = vec![0.0; np];
            let mut hs = vec![0.0; np * np];
            for i in 0..np {
                let cp = self.cost(&shift(&th, &[(i, h[i])]), k)?;
                let cm = self.cost(&shift(&th, &[(i, -h[i])]), k)?;
                gr[i] = (cp - cm) / (2.0 * h[i]);
                hs[i * np + i] = (cp - 2.0 * c0 + cm) / (h[i] * h[i]);
            }
            for i in 0..np {
                for j in i + 1..np {
                    let pp = self.cost(&shift(&th, &[(i, h[i]), (j, h[j])]), k)?;
                    let pm = self.cost(&shift(&th, &[(i, h[i]), (j, -h[j])]), k)?;
                    let mp = self.cost(&shift(&th, &[(i, -h[i]), (j, h[j])]), k)?;
                    let mm = self.cost(&shift(&th, &[(i, -h[i]), (j, -h[j])]), k)?;
                    let v = (pp - pm - mp + mm) / (4.0 * h[i] * h[j]);
                    hs[i * np + j] = v;
                    hs[j * np + i] = v;
                }
            }
            let mut ok = false;
            let mut last_step: Option<Vec<f64>> = None;
            for _ in 0..8 {
                let mut hd = hs.clone();
                for i in 0..np {
                    hd[i * np + i] += lam * hs[i * np + i].abs().max(1e-30);
                }
                let Some(sol) = solve_real(&hd, &gr, np) else {
                    lam *= 10.0;
                    continue;
                };
                let mut stp: Vec<f64> = sol.iter().map(|v| -v).collect();
                for i in 0..k {
                    let b = 0.01 * th[i];
                    stp[i] = stp[i].clamp(-b.abs(), b.abs());
                }
                for v in stp.iter_mut().skip(k) {
                    *v = v.clamp(-0.5, 0.5);
                }
                let trial: Vec<f64> = th.iter().zip(&stp).map(|(a, b)| a + b).collect();
                let c1 = self.cost(&trial, k)?;
                last_step = Some(stp);
                if c1 < c0 {
                    th = trial;
                    c0 = c1;
                    lam = (lam / 3.0).max(1e-6);
                    ok = true;
                    break;
                }
                lam *= 10.0;
            }
            let Some(stp) = last_step else { break };
            let rel = (0..k).map(|i| (stp[i] / th[i]).abs()).fold(0.0, f64::max);
            if !ok || rel < 1e-7 {
                break;
            }
        }
        let fr: Vec<f64> = th[..k].to_vec();
        let zr: Vec<f64> = th[k..].iter().map(|v| v.exp()).collect();
        let (_, a) = self.lsq(&fr, &zr)?;
        Ok((fr, zr, a))
    }
}

/// Local quadratic least squares over `+- s` lines of each element of a
/// line-major stack (`size` values a line), keeping the fitted value at the
/// centre line.
pub fn smooth_grid(grid: &[f64], data: &[C64], size: usize, s: usize) -> Vec<C64> {
    let l = grid.len();
    let step = (grid[1] - grid[0]).max(1e-12);
    let mut out = vec![ZERO; data.len()];
    let mut w = Vec::new();
    for k in 0..l {
        let a = k.saturating_sub(s);
        let b = (k + s + 1).min(l);
        w.clear();
        if b - a >= 3 {
            let xs: Vec<f64> = grid[a..b].iter().map(|x| (x - grid[k]) / step).collect();
            let mut vtv = [0.0f64; 9];
            for &x in &xs {
                let v = [1.0, x, x * x];
                for i in 0..3 {
                    for j in 0..3 {
                        vtv[i * 3 + j] += v[i] * v[j];
                    }
                }
            }
            let u = solve_real(&vtv, &[1.0, 0.0, 0.0], 3).unwrap_or_else(|| vec![0.0; 3]);
            w.extend(xs.iter().map(|&x| u[0] + u[1] * x + u[2] * x * x));
        } else {
            let n = (b - a) as f64;
            w.extend((a..b).map(|_| 1.0 / n));
        }
        for el in 0..size {
            let mut acc = ZERO;
            for (j, wj) in w.iter().enumerate() {
                acc += data[(a + j) * size + el] * *wj;
            }
            out[k * size + el] = acc;
        }
    }
    out
}

/// Which model the carry and the correction read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ModelChoice {
    /// The identification's modes exact, the remainder smoothed over `+- lines`.
    Modal {
        /// Half-width of the smoothing, lines.
        lines: usize,
    },
    /// The whole identification smoothed over `+- lines` (no modal fit).
    Smoothed {
        /// Half-width of the smoothing, lines.
        lines: usize,
    },
    /// The identification as measured.
    Identification,
}

impl Default for ModelChoice {
    fn default() -> Self {
        ModelChoice::Modal { lines: 8 }
    }
}

/// The model on the identification's grid, read by linear interpolation.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    grid: Vec<f64>,
    data: Vec<C64>,
    size: usize,
    /// The modes found (empty unless the choice is modal).
    pub modes: ModalForm,
}

impl Model {
    /// Build the chosen model over `band` (Hz).
    pub fn build(
        id: &Identification,
        band: (f64, f64),
        choice: ModelChoice,
    ) -> Result<Model, String> {
        let (l, n, m) = id.response.dims();
        let size = n * m;
        let g = &id.grid_hz;
        let raw = id.response.data();
        let none = ModalForm {
            f_hz: Vec::new(),
            zeta: Vec::new(),
            residues: Vec::new(),
        };
        let (data, modes) = match choice {
            ModelChoice::Identification => (raw.to_vec(), none),
            ModelChoice::Smoothed { lines } => (smooth_grid(g, raw, size, lines), none),
            ModelChoice::Modal { lines } => {
                let mf = ModalForm::fit(id, band)?;
                (modal_smoothed(id, &mf, lines), mf)
            }
        };
        Ok(Model {
            grid: g.clone(),
            data,
            size,
            modes,
        })
    }

    /// The modal-smoothed model from modes already found (a fit made once and
    /// kept, or made elsewhere).
    pub fn from_modes(
        id: &Identification,
        modes: ModalForm,
        lines: usize,
    ) -> Result<Model, String> {
        let (_, n, m) = id.response.dims();
        if modes.f_hz.len() != modes.zeta.len() || modes.residues.len() != modes.f_hz.len() * n * m
        {
            return Err("the modes do not match the identification".into());
        }
        Ok(Model {
            grid: id.grid_hz.clone(),
            data: modal_smoothed(id, &modes, lines),
            size: n * m,
            modes,
        })
    }

    /// The model at `f_hz`, `controls x drives` row-major.
    pub fn at(&self, f_hz: f64, out: &mut [C64]) {
        interp_stack(&self.grid, &self.data, self.size, f_hz, out);
    }
}

/// `sum_r A_r q_r + smooth[H - sum_r A_r q_r]` on the identification's grid.
fn modal_smoothed(id: &Identification, mf: &ModalForm, lines: usize) -> Vec<C64> {
    let (l, n, m) = id.response.dims();
    let size = n * m;
    let g = &id.grid_hz;
    let mut modal = vec![ZERO; l * size];
    for (k, &f) in g.iter().enumerate() {
        for r in 0..mf.modes() {
            let q = q_mode(f, mf.f_hz[r], mf.zeta[r]);
            let a = &mf.residues[r * size..(r + 1) * size];
            for el in 0..size {
                modal[k * size + el] += q * a[el];
            }
        }
    }
    let rem: Vec<C64> = id
        .response
        .data()
        .iter()
        .zip(&modal)
        .map(|(h, q)| h - q)
        .collect();
    let mut data = smooth_grid(g, &rem, size, lines);
    for (d, q) in data.iter_mut().zip(&modal) {
        *d += q;
    }
    data
}
