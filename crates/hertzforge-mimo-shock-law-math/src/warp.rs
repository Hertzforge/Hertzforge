//! The warp follower: the plant at a level is the identified matrix seen at a
//! complex frequency (`Derivations.md` D-R2-2 and D20), with no modal fit.
//!
//! The model at a level is
//!
//! `Hw(f) = Ph(f) Hd(sigma(f) + j 2 pi a(f) f)`,
//!
//! where `Hd = Ph^-1 H` is the identification with each control's delay
//! removed, `a(f)` a smooth stretch of the frequency axis (every resonance
//! moves with it) and `sigma(f) >= 0` a smooth decay (an exponential window
//! `exp(-sigma t)` on the identified impulse response, which moves every pole
//! left by `sigma`: more damping). Both are hat-function expansions on
//! log-frequency knots, shared by every element of the matrix. `Hd` at a complex
//! frequency is read from a bank of windowed copies of the identification,
//! interpolated bilinearly in the decay and in the line index.
//!
//! After every pulse the knot parameters `x = [1/a_k - 1, sigma_k]` are fitted
//! to the level's pulses by Gauss-Newton (the decay's derivative follows from
//! the stretch's by the Cauchy-Riemann equations), in scaled units, whitened by
//! the noise measured on lines the pulse does not excite, with a ridge toward
//! the starting point, second-difference smoothing and step halving on the
//! cost. Each level's fit is stored against its amplitude scale (the
//! identification is the first point when its scale is given); the first
//! pulse of a new level gets the parameters extrapolated linearly in scale
//! through the last two levels. At the same level a same-level secant refines
//! the warped model (`Derivations.md` D-R2-3).
//!
//! An optional significance gate refuses a fit whose improvement on the
//! level's pulses is no larger than fitting noise alone would give
//! (`Derivations.md` D20).

// Per-line matrix arithmetic indexes several arrays by the same row and
// column; index loops are the clearer form here.
#![allow(clippy::needless_range_loop)]

use crate::lsq::{hpd_solve, spd_solve};
use crate::provider::{
    prediction_residual, secant_correct, ModelProvider, ModelReport, ModelSource, Observation,
    SecantOptions,
};
use crate::spectrum::{irfft, rfft};
use crate::{level_scale, MatrixStack, C64};
use std::f64::consts::{PI, TAU};

/// The same-level refinement on top of the warped model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WarpSecant {
    /// The minimum-change multi-secant with a noise-aware ridge per line,
    /// `mu = nu^2 / tau^2` (`Derivations.md` D-R2-3).
    NoiseRidge,
    /// The plain multi-secant of [`secant_correct`] with these options
    /// (`Derivations.md` D14 and D18).
    Plain(SecantOptions),
}

/// Options of [`WarpModel`].
#[derive(Clone, Debug, PartialEq)]
pub struct WarpOptions {
    /// Lowest frequency of the warped band, Hz (lines strictly above it).
    pub band_lo_hz: f64,
    /// Highest frequency of the warped band, Hz (lines strictly below it).
    pub band_hi_hz: f64,
    /// Knots per octave of the stretch and decay fields.
    pub knots_per_octave: f64,
    /// Largest decay in the bank, rad/s.
    pub sigma_max: f64,
    /// Decay step of the bank, rad/s.
    pub sigma_step: f64,
    /// Highest line kept in the bank, Hz.
    pub bank_hi_hz: f64,
    /// Band where each control's delay is read from the phase slope, Hz.
    pub delay_band_hz: (f64, f64),
    /// Band of lines the pulse does not excite, where the noise is measured,
    /// Hz.
    pub noise_band_hz: (f64, f64),
    /// Floor of the noise variance, relative to the response's mean power in
    /// the band.
    pub noise_floor_rel: f64,
    /// Gauss-Newton iterations a pulse.
    pub iterations: usize,
    /// Weight of the ridge toward the starting parameters (scaled units).
    pub ridge: f64,
    /// Weight of the second-difference smoothing of each field (scaled units).
    pub smooth: f64,
    /// Scale of `1/a - 1`.
    pub scale_inv_a: f64,
    /// Scale of the decay, rad/s.
    pub scale_sigma: f64,
    /// Largest `|1/a - 1|` at a knot.
    pub max_inv_a_change: f64,
    /// Whether the first pulse of a new level gets parameters extrapolated
    /// linearly in amplitude scale (else the last level's are held).
    pub extrapolate: bool,
    /// The amplitude scale of the identification. When set, it is the first
    /// point of the level history (parameters zero).
    pub identification_scale: Option<f64>,
    /// The same-level refinement (`None`: the warped model alone).
    pub secant: Option<WarpSecant>,
    /// Width in lines of the moving average of the noise-ridge's prior.
    pub secant_smooth_lines: usize,
    /// The significance gate: a fit is kept only if it lowers the whitened
    /// misfit of the level's pulses by more than this many times half its
    /// effective number of parameters (`None`: off).
    pub gate: Option<f64>,
}

impl Default for WarpOptions {
    fn default() -> Self {
        WarpOptions {
            band_lo_hz: 80.0,
            band_hi_hz: 2400.0,
            knots_per_octave: 6.0,
            sigma_max: 800.0,
            sigma_step: 10.0,
            bank_hi_hz: 3400.0,
            delay_band_hz: (3000.0, 6000.0),
            noise_band_hz: (6000.0, 9500.0),
            noise_floor_rel: 1e-6,
            iterations: 6,
            ridge: 1.0,
            smooth: 1.0,
            scale_inv_a: 0.05,
            scale_sigma: 100.0,
            max_inv_a_change: 0.25,
            extrapolate: true,
            identification_scale: None,
            secant: Some(WarpSecant::NoiseRidge),
            secant_smooth_lines: 9,
            gate: None,
        }
    }
}

type Pair = (Vec<Vec<C64>>, Vec<Vec<C64>>);

/// One band line's place in the knot expansion: two knots and their weights.
#[derive(Clone, Copy, Debug)]
struct Hat {
    line: usize,
    k: [usize; 2],
    w: [f64; 2],
}

/// The warp follower (module documentation).
#[derive(Clone)]
pub struct WarpModel {
    base: MatrixStack,
    fs_hz: f64,
    opts: WarpOptions,
    n_c: usize,
    n_d: usize,
    df: f64,
    delays_s: Vec<f64>,
    /// `exp(-j w tau_c)` on the band lines, `[band line][control]`.
    ph: Vec<Vec<C64>>,
    /// The bank, `((j * nl + i) * n_c + r) * n_d + c`.
    bank: Vec<C64>,
    nl: usize,
    nsig: usize,
    hats: Vec<Hat>,
    nk: usize,
    resid_band: Vec<bool>,
    store: Vec<(f64, Vec<f64>)>,
    cur: Vec<f64>,
    level: Option<f64>,
    pairs: Vec<Pair>,
    nu2: f64,
    current: MatrixStack,
    at_level: Option<MatrixStack>,
    report: ModelReport,
    refused: usize,
    last_dof: f64,
    last_costs: Vec<f64>,
}

/// numpy's `unwrap` (discontinuity pi).
fn unwrap(p: &mut [f64]) {
    let mut corr = 0.0;
    for i in 1..p.len() {
        let d = p[i] - (p[i - 1] - corr);
        let mut dd = (d + PI).rem_euclid(TAU) - PI;
        if dd == -PI && d > 0.0 {
            dd = PI;
        }
        let c = if d.abs() < PI { 0.0 } else { dd - d };
        corr += c;
        p[i] += corr;
    }
}

/// Each control's delay from the weighted phase slope of its row of `h` over
/// `band_hz` (well above the modes every element is a real constant times
/// `exp(-j w tau)`), pooled over the row (`Derivations.md` D-R2-2 step 5).
pub fn row_delays(h: &MatrixStack, fs_hz: f64, band_hz: (f64, f64)) -> Vec<f64> {
    let (lines, n_c, n_d) = h.dims();
    let df = fs_hz / (2 * (lines - 1)) as f64;
    let idx: Vec<usize> = (0..lines)
        .filter(|&k| {
            let f = k as f64 * df;
            f > band_hz.0 && f < band_hz.1
        })
        .collect();
    let mut out = vec![0.0; n_c];
    if idx.len() < 2 {
        return out;
    }
    for (r, tau) in out.iter_mut().enumerate() {
        let (mut num, mut den) = (0.0, 0.0);
        for c in 0..n_d {
            let vals: Vec<C64> = idx.iter().map(|&k| h.get(k, r, c)).collect();
            let wt: Vec<f64> = vals.iter().map(|v| v.norm_sqr()).collect();
            let sw: f64 = wt.iter().sum();
            if sw <= 0.0 {
                continue;
            }
            let mut ph: Vec<f64> = vals.iter().map(|v| v.arg()).collect();
            unwrap(&mut ph);
            let w: Vec<f64> = idx.iter().map(|&k| TAU * k as f64 * df).collect();
            let wm = w.iter().zip(&wt).map(|(a, b)| a * b).sum::<f64>() / sw;
            let pm = ph.iter().zip(&wt).map(|(a, b)| a * b).sum::<f64>() / sw;
            let (mut sxy, mut sxx) = (0.0, 0.0);
            for i in 0..w.len() {
                sxy += wt[i] * (w[i] - wm) * (ph[i] - pm);
                sxx += wt[i] * (w[i] - wm) * (w[i] - wm);
            }
            if sxx > 0.0 {
                num += -(sxy / sxx) * sw;
                den += sw;
            }
        }
        if den > 0.0 {
            *tau = num / den;
        }
    }
    out
}

/// Moving average across lines of odd `width`, zero beyond the ends (numpy's
/// `convolve(..., "same")`).
fn moving_average(x: &[f64], width: usize) -> Vec<f64> {
    if width <= 1 {
        return x.to_vec();
    }
    let h = width / 2;
    let n = x.len();
    let mut out = vec![0.0; n];
    for (i, o) in out.iter_mut().enumerate() {
        let lo = i.saturating_sub(h);
        let hi = (i + h).min(n - 1);
        *o = x[lo..=hi].iter().sum::<f64>() / width as f64;
    }
    out
}

/// What a fit returns: the parameters, the whitening variance, the normal
/// matrix and the full system matrix at the last linearization, and the
/// regularized cost after each accepted step (the first is the start's).
struct FitOut {
    x: Vec<f64>,
    sig2: f64,
    ata: Vec<f64>,
    q: Vec<f64>,
    costs: Vec<f64>,
}

/// What one evaluation of the warped model gives on a band line.
struct Eval {
    h: Vec<C64>,
    dh: Vec<C64>,
    a: f64,
}

impl WarpModel {
    /// The warp follower of the identification `base` (lines of a record at
    /// `fs_hz`); `resid_band` is where its prediction residual is reported.
    pub fn new(
        base: MatrixStack,
        resid_band: Vec<bool>,
        fs_hz: f64,
        opts: WarpOptions,
    ) -> Result<Self, String> {
        let (lines, n_c, n_d) = base.dims();
        if lines < 3 || resid_band.len() != lines {
            return Err("the base model and the band do not agree".into());
        }
        if !(opts.sigma_step > 0.0 && opts.sigma_max >= opts.sigma_step) {
            return Err("the decay bank needs a positive step below its maximum".into());
        }
        if !(opts.band_lo_hz > 0.0 && opts.band_hi_hz > opts.band_lo_hz) {
            return Err("the warped band must be a positive interval".into());
        }
        let n = 2 * (lines - 1);
        let df = fs_hz / n as f64;
        let delays_s = row_delays(&base, fs_hz, opts.delay_band_hz);
        // The delay-free identification and its circular impulse responses.
        let mut hd = base.clone();
        for f in 0..lines {
            let w = TAU * f as f64 * df;
            for r in 0..n_c {
                let back = C64::from_polar(1.0, w * delays_s[r]);
                for c in 0..n_d {
                    let v = base.get(f, r, c) * back;
                    hd.set(f, r, c, v);
                }
            }
        }
        for r in 0..n_c {
            for c in 0..n_d {
                let v = hd.get(lines - 1, r, c);
                hd.set(lines - 1, r, c, C64::new(v.re, 0.0));
            }
        }
        // nl = searchsorted(fr, bank_hi) + 1.
        let first_at = (0..lines)
            .find(|&k| k as f64 * df >= opts.bank_hi_hz)
            .unwrap_or(lines - 1);
        let nl = (first_at + 1).min(lines);
        let nsig =
            ((opts.sigma_max + opts.sigma_step / 2.0) / opts.sigma_step).floor() as usize + 1;
        let mut bank = vec![C64::new(0.0, 0.0); nsig * nl * n_c * n_d];
        let t: Vec<f64> = (0..n).map(|k| k as f64 / fs_hz).collect();
        let mut buf = vec![0.0; n];
        for r in 0..n_c {
            for c in 0..n_d {
                let col: Vec<C64> = (0..lines).map(|f| hd.get(f, r, c)).collect();
                let h = irfft(&col, n)?;
                for j in 0..nsig {
                    let sg = j as f64 * opts.sigma_step;
                    for k in 0..n {
                        buf[k] = h[k] * (-sg * t[k]).exp();
                    }
                    let spec = rfft(&buf)?;
                    for (i, v) in spec.iter().take(nl).enumerate() {
                        bank[((j * nl + i) * n_c + r) * n_d + c] = *v;
                    }
                }
            }
        }
        // Knots and hats on the band lines.
        let k0 = opts.band_lo_hz.log2();
        let k1 = opts.band_hi_hz.log2();
        let step = 1.0 / opts.knots_per_octave;
        let mut knots = Vec::new();
        let mut kv = k0;
        let mut i = 0usize;
        while kv <= k1 + 1e-9 {
            knots.push(kv);
            i += 1;
            kv = k0 + i as f64 * step;
        }
        let nk = knots.len();
        if nk < 3 {
            return Err("the warped band needs at least three knots".into());
        }
        let mut hats = Vec::new();
        let mut ph = Vec::new();
        for f in 0..lines {
            let fr = f as f64 * df;
            if !(fr > opts.band_lo_hz && fr < opts.band_hi_hz) {
                continue;
            }
            let lf = fr.max(1.0).log2();
            let mut ks = [0usize; 2];
            let mut ws = [0.0f64; 2];
            let mut m = 0;
            for (k, kn) in knots.iter().enumerate() {
                let wv = (1.0 - (lf - kn).abs() * opts.knots_per_octave).max(0.0);
                if wv > 0.0 && m < 2 {
                    ks[m] = k;
                    ws[m] = wv;
                    m += 1;
                }
            }
            if m == 1 {
                ks[1] = ks[0];
                ws[1] = 0.0;
            }
            hats.push(Hat {
                line: f,
                k: ks,
                w: ws,
            });
            let w = TAU * fr;
            ph.push(
                (0..n_c)
                    .map(|r| C64::from_polar(1.0, -w * delays_s[r]))
                    .collect(),
            );
        }
        let store = match opts.identification_scale {
            Some(s) => vec![(s, vec![0.0; 2 * nk])],
            None => Vec::new(),
        };
        Ok(WarpModel {
            current: base.clone(),
            base,
            fs_hz,
            n_c,
            n_d,
            df,
            delays_s,
            ph,
            bank,
            nl,
            nsig,
            hats,
            nk,
            resid_band,
            store,
            cur: vec![0.0; 2 * nk],
            level: None,
            pairs: Vec::new(),
            nu2: 0.0,
            at_level: None,
            report: ModelReport::default(),
            refused: 0,
            last_dof: f64::NAN,
            last_costs: Vec::new(),
            opts,
        })
    }

    /// Replaces the same-level refinement and the significance gate (the
    /// other options shape the bank and are fixed at construction). Meant for
    /// a copy of a follower made before its first pulse.
    pub fn set_refinement(&mut self, secant: Option<WarpSecant>, gate: Option<f64>) {
        self.opts.secant = secant;
        self.opts.gate = gate;
    }

    /// The delay removed from each control before warping, seconds.
    pub fn delays(&self) -> &[f64] {
        &self.delays_s
    }

    /// The knot parameters now in force: `1/a - 1` at each knot, then the
    /// decay at each knot (rad/s).
    pub fn parameters(&self) -> &[f64] {
        &self.cur
    }

    /// The level history: amplitude scale and parameters.
    pub fn history(&self) -> &[(f64, Vec<f64>)] {
        &self.store
    }

    /// How many fits the significance gate refused, and the effective number
    /// of parameters of the last fit.
    pub fn gate_counts(&self) -> (usize, f64) {
        (self.refused, self.last_dof)
    }

    /// The regularized cost of the last fit after each accepted step, the
    /// first entry its starting point's.
    pub fn last_costs(&self) -> &[f64] {
        &self.last_costs
    }

    /// Bytes held by the bank of windowed copies.
    pub fn bank_bytes(&self) -> usize {
        self.bank.len() * std::mem::size_of::<C64>()
    }

    /// The sample rate the model was built for, Hz.
    pub fn fs_hz(&self) -> f64 {
        self.fs_hz
    }

    /// Each knot's frequency, Hz, in the order of [`WarpModel::parameters`].
    pub fn knot_hz(&self) -> Vec<f64> {
        let k0 = self.opts.band_lo_hz.log2();
        (0..self.nk)
            .map(|k| 2f64.powf(k0 + k as f64 / self.opts.knots_per_octave))
            .collect()
    }

    fn fields(&self, x: &[f64], hat: &Hat) -> (f64, f64) {
        let nk = self.nk;
        let ainv = 1.0 + hat.w[0] * x[hat.k[0]] + hat.w[1] * x[hat.k[1]];
        let sg = hat.w[0] * x[nk + hat.k[0]] + hat.w[1] * x[nk + hat.k[1]];
        (1.0 / ainv, sg)
    }

    /// The warped model on band line `hi` (index into the hats), and its
    /// derivative in the warped frequency (per Hz) when `deriv`.
    fn eval(&self, x: &[f64], hi: usize, deriv: bool) -> Eval {
        let hat = &self.hats[hi];
        let (a, sg) = self.fields(x, hat);
        let nu = hat.line as f64 * a;
        let smax = (self.nsig - 1) as f64 * self.opts.sigma_step;
        let sgv = sg.clamp(0.0, smax - 1e-9);
        let js = sgv / self.opts.sigma_step;
        let j0 = (js.floor() as usize).min(self.nsig - 2);
        let tj = js - j0 as f64;
        let i0f = nu.floor();
        let i0 = if i0f < 0.0 {
            0usize
        } else {
            (i0f as usize).min(self.nl - 3)
        };
        let ti = nu - i0 as f64;
        let e = self.n_c * self.n_d;
        let at = |j: usize, i: usize| &self.bank[(j * self.nl + i) * e..(j * self.nl + i + 1) * e];
        let (b00, b01, b10, b11) = (
            at(j0, i0),
            at(j0 + 1, i0),
            at(j0, i0 + 1),
            at(j0 + 1, i0 + 1),
        );
        let mut h = vec![C64::new(0.0, 0.0); e];
        let mut dh = if deriv {
            vec![C64::new(0.0, 0.0); e]
        } else {
            Vec::new()
        };
        for r in 0..self.n_c {
            let p = self.ph[hi][r];
            for c in 0..self.n_d {
                let q = r * self.n_d + c;
                let g0 = b00[q] * (1.0 - tj) + b01[q] * tj;
                let g1 = b10[q] * (1.0 - tj) + b11[q] * tj;
                h[q] = (g0 * (1.0 - ti) + g1 * ti) * p;
                if deriv {
                    dh[q] = (g1 - g0) / self.df * p;
                }
            }
        }
        Eval { h, dh, a }
    }

    /// The warped model on every line (the identification outside the band).
    pub fn model_at(&self, x: &[f64]) -> MatrixStack {
        let mut m = self.base.clone();
        for hi in 0..self.hats.len() {
            let ev = self.eval(x, hi, false);
            m.line_mut(self.hats[hi].line).copy_from_slice(&ev.h);
        }
        m
    }

    /// The parameters predicted for amplitude scale `s`.
    pub fn predict(&self, s: f64) -> Vec<f64> {
        if let Some(p) = self.store.iter().find(|p| (p.0 - s).abs() < 1e-9) {
            return p.1.clone();
        }
        if self.store.is_empty() {
            return vec![0.0; 2 * self.nk];
        }
        let below: Vec<&(f64, Vec<f64>)> = {
            let b: Vec<_> = self.store.iter().filter(|p| p.0 < s).collect();
            if b.is_empty() {
                self.store.iter().collect()
            } else {
                b
            }
        };
        let last = below[below.len() - 1];
        if !self.opts.extrapolate || below.len() < 2 {
            return last.1.clone();
        }
        let prev = below[below.len() - 2];
        let t = (s - last.0) / (last.0 - prev.0);
        let mut x: Vec<f64> = last
            .1
            .iter()
            .zip(&prev.1)
            .map(|(a, b)| a + (a - b) * t)
            .collect();
        // A stretch may not fold the axis; a decay may not go negative.
        let nk = self.nk;
        for v in x[..nk].iter_mut() {
            *v = v.clamp(-0.5, 0.5);
        }
        for v in x[nk..].iter_mut() {
            *v = v.max(0.0);
        }
        x
    }

    fn store_put(&mut self, s: f64, x: Vec<f64>) {
        if let Some(p) = self.store.iter_mut().find(|p| (p.0 - s).abs() < 1e-9) {
            p.1 = x;
        } else {
            self.store.push((s, x));
            self.store
                .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        }
    }

    /// Sum over the level's pulses and the band of `|Y - Hw(x) D|^2`.
    fn misfit(&self, x: &[f64]) -> f64 {
        let mut acc = 0.0;
        for hi in 0..self.hats.len() {
            let ev = self.eval(x, hi, false);
            let f = self.hats[hi].line;
            for (d, y) in &self.pairs {
                for r in 0..self.n_c {
                    let mut hd = C64::new(0.0, 0.0);
                    for c in 0..self.n_d {
                        hd += ev.h[r * self.n_d + c] * d[c][f];
                    }
                    acc += (y[r][f] - hd).norm_sqr();
                }
            }
        }
        acc
    }

    fn penalty(&self, z: &[f64], zp: &[f64]) -> f64 {
        let nk = self.nk;
        let mut pen = 0.0;
        for half in 0..2 {
            for k in 0..nk - 2 {
                let d2 = z[half * nk + k] - 2.0 * z[half * nk + k + 1] + z[half * nk + k + 2];
                pen += 0.5 * self.opts.smooth * d2 * d2;
            }
        }
        pen + 0.5
            * self.opts.ridge
            * z.iter()
                .zip(zp)
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f64>()
    }

    fn scales(&self) -> Vec<f64> {
        let nk = self.nk;
        (0..2 * nk)
            .map(|i| {
                if i < nk {
                    self.opts.scale_inv_a
                } else {
                    self.opts.scale_sigma
                }
            })
            .collect()
    }

    /// The smoothing matrix `smooth * D2^T D2` on each half.
    fn smoothing_matrix(&self) -> Vec<f64> {
        let nk = self.nk;
        let np = 2 * nk;
        let mut m = vec![0.0; np * np];
        for half in 0..2 {
            for k in 0..nk - 2 {
                let idx = [half * nk + k, half * nk + k + 1, half * nk + k + 2];
                let coef = [1.0, -2.0, 1.0];
                for a in 0..3 {
                    for b in 0..3 {
                        m[idx[a] * np + idx[b]] += self.opts.smooth * coef[a] * coef[b];
                    }
                }
            }
        }
        m
    }

    /// Gauss-Newton from `prior` on the level's pulses (`Derivations.md`
    /// D-R2-2 step 5). Returns the parameters, the whitening variance, and
    /// the normal matrix and full system matrix at the last linearization.
    fn fit(&self, prior: &[f64], ysig2: f64) -> FitOut {
        let nk = self.nk;
        let np = 2 * nk;
        let tau = self.scales();
        let sig2 = (self.nu2 / 2.0).max(self.opts.noise_floor_rel * ysig2);
        let msm = self.smoothing_matrix();
        let zp: Vec<f64> = prior.iter().zip(&tau).map(|(a, b)| a / b).collect();
        let cost = |xx: &[f64]| -> f64 {
            let z: Vec<f64> = xx.iter().zip(&tau).map(|(a, b)| a / b).collect();
            self.misfit(xx) / (2.0 * sig2) + self.penalty(&z, &zp)
        };
        let mut x = prior.to_vec();
        let mut j0 = cost(&x);
        let mut costs = vec![j0];
        let mut last_ata = vec![0.0; np * np];
        let mut last_q = vec![0.0; np * np];
        let smax = (self.nsig - 1) as f64 * self.opts.sigma_step;
        for _ in 0..self.opts.iterations {
            let mut ata = vec![0.0; np * np];
            let mut atb = vec![0.0; np];
            for hi in 0..self.hats.len() {
                let ev = self.eval(&x, hi, true);
                let hat = self.hats[hi];
                let f = hat.line;
                let fr = f as f64 * self.df;
                let ca_fac = -fr * ev.a * ev.a;
                let cs_fac = C64::new(0.0, -1.0 / TAU);
                let idx = [hat.k[0], hat.k[1], nk + hat.k[0], nk + hat.k[1]];
                for (d, y) in &self.pairs {
                    for r in 0..self.n_c {
                        let mut hd = C64::new(0.0, 0.0);
                        let mut g = C64::new(0.0, 0.0);
                        for c in 0..self.n_d {
                            hd += ev.h[r * self.n_d + c] * d[c][f];
                            g += ev.dh[r * self.n_d + c] * d[c][f];
                        }
                        let res = y[r][f] - hd;
                        let ca = g * ca_fac;
                        let cs = g * cs_fac;
                        let cols = [
                            ca * hat.w[0] * tau[idx[0]],
                            ca * hat.w[1] * tau[idx[1]],
                            cs * hat.w[0] * tau[idx[2]],
                            cs * hat.w[1] * tau[idx[3]],
                        ];
                        for a in 0..4 {
                            atb[idx[a]] += (cols[a].conj() * res).re / sig2;
                            for b in 0..4 {
                                ata[idx[a] * np + idx[b]] += (cols[a].conj() * cols[b]).re / sig2;
                            }
                        }
                    }
                }
            }
            let z: Vec<f64> = x.iter().zip(&tau).map(|(a, b)| a / b).collect();
            let mut q = ata.clone();
            let mut rhs = atb.clone();
            for a in 0..np {
                q[a * np + a] += self.opts.ridge;
                let mut mz = 0.0;
                for b in 0..np {
                    q[a * np + b] += msm[a * np + b];
                    mz += msm[a * np + b] * z[b];
                }
                rhs[a] -= self.opts.ridge * (z[a] - zp[a]) + mz;
            }
            last_ata.clone_from(&ata);
            last_q.clone_from(&q);
            let mut st = match spd_solve(&q, np, &rhs, 0.0) {
                Ok(s) => s,
                Err(_) => break,
            };
            let mut accepted: Option<(Vec<f64>, f64)> = None;
            for _ in 0..8 {
                let mut xn: Vec<f64> = x
                    .iter()
                    .zip(st.iter().zip(&tau))
                    .map(|(a, (s, t))| a + s * t)
                    .collect();
                for v in xn[..nk].iter_mut() {
                    *v = v.clamp(-self.opts.max_inv_a_change, self.opts.max_inv_a_change);
                }
                for v in xn[nk..].iter_mut() {
                    *v = v.clamp(0.0, smax);
                }
                let jn = cost(&xn);
                if jn <= j0 {
                    accepted = Some((xn, jn));
                    break;
                }
                st.iter_mut().for_each(|v| *v *= 0.5);
            }
            match accepted {
                Some((xn, jn)) => {
                    x = xn;
                    j0 = jn;
                    costs.push(jn);
                }
                None => break,
            }
        }
        FitOut {
            x,
            sig2,
            ata: last_ata,
            q: last_q,
            costs,
        }
    }

    /// The noise-ridged same-level secant on top of `hw` (`Derivations.md`
    /// D-R2-3), over every line.
    fn noise_ridge_secant(&self, hw: &MatrixStack) -> MatrixStack {
        let (lines, n_c, n_d) = hw.dims();
        let m = self.pairs.len();
        let mut out = hw.clone();
        if m == 0 {
            return out;
        }
        let zero = C64::new(0.0, 0.0);
        let mut rnorm = vec![0.0; lines];
        let mut dnorm = vec![0.0; lines];
        let mut tr = vec![0.0; lines];
        let mut resid = vec![zero; lines * n_c * m];
        for f in 0..lines {
            let l = hw.line(f);
            for (p, (d, y)) in self.pairs.iter().enumerate() {
                for r in 0..n_c {
                    let mut hd = zero;
                    for c in 0..n_d {
                        hd += l[r * n_d + c] * d[c][f];
                    }
                    let e = y[r][f] - hd;
                    resid[(f * n_c + r) * m + p] = e;
                    rnorm[f] += e.norm_sqr();
                }
                for dc in d.iter().take(n_d) {
                    dnorm[f] += dc[f].norm_sqr();
                }
            }
            tr[f] = dnorm[f];
        }
        let w = self.opts.secant_smooth_lines;
        let rs = moving_average(&rnorm, w);
        let ds = moving_average(&dnorm, w);
        let trmax = tr.iter().fold(0.0f64, |a, &b| a.max(b));
        for f in 0..lines {
            if tr[f] <= 1e-12 * trmax {
                continue;
            }
            let tau2 = rs[f] / (n_c as f64 * ds[f]).max(1e-300);
            let mu = (self.nu2 / tau2.max(1e-300)).max(1e-10 * tr[f]);
            let mut g = vec![zero; m * m];
            for a in 0..m {
                for b in 0..m {
                    g[a * m + b] = (0..n_d)
                        .map(|c| self.pairs[a].0[c][f].conj() * self.pairs[b].0[c][f])
                        .sum();
                }
                g[a * m + a] += C64::new(mu, 0.0);
            }
            let mut rhs = vec![zero; m * n_d];
            for a in 0..m {
                for c in 0..n_d {
                    rhs[a * n_d + c] = self.pairs[a].0[c][f].conj();
                }
            }
            let x = match hpd_solve(&g, m, &rhs, n_d) {
                Ok(x) => x,
                Err(_) => continue,
            };
            let line = out.line_mut(f);
            for r in 0..n_c {
                for c in 0..n_d {
                    let mut acc = zero;
                    for a in 0..m {
                        acc += resid[(f * n_c + r) * m + a] * x[a * n_d + c];
                    }
                    line[r * n_d + c] += acc;
                }
            }
        }
        out
    }

    fn refine(&self, hw: MatrixStack) -> MatrixStack {
        match &self.opts.secant {
            None => hw,
            Some(WarpSecant::NoiseRidge) => self.noise_ridge_secant(&hw),
            Some(WarpSecant::Plain(so)) => secant_correct(&hw, &self.pairs, so),
        }
    }

    /// The noise variance per line of the last pulse's normalized response,
    /// from the lines it does not excite: the median power over those lines
    /// and every control, divided by `ln 2` (the median of an exponential
    /// variable is `ln 2` times its mean).
    fn noise_variance(&self, responses: &[Vec<C64>]) -> f64 {
        let (lo, hi) = self.opts.noise_band_hz;
        let mut p: Vec<f64> = Vec::new();
        for row in responses {
            for (k, v) in row.iter().enumerate() {
                let f = k as f64 * self.df;
                if f > lo && f < hi {
                    p.push(v.norm_sqr());
                }
            }
        }
        if p.is_empty() {
            return 0.0;
        }
        p.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = p.len();
        let med = if n % 2 == 1 {
            p[n / 2]
        } else {
            0.5 * (p[n / 2 - 1] + p[n / 2])
        };
        med / std::f64::consts::LN_2
    }

    /// The effective number of parameters `tr((A^T A + R)^-1 A^T A)`.
    fn effective_dof(ata: &[f64], q: &[f64], np: usize) -> f64 {
        let mut tr = 0.0;
        for col in 0..np {
            let b: Vec<f64> = (0..np).map(|i| ata[i * np + col]).collect();
            if let Ok(x) = spd_solve(q, np, &b, 0.0) {
                tr += x[col];
            }
        }
        tr
    }
}

impl ModelProvider for WarpModel {
    fn current(&self) -> &MatrixStack {
        &self.current
    }

    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String> {
        if obs.drives.len() != self.n_d || obs.responses.len() != self.n_c {
            return Err("the pulse does not match the model's shape".into());
        }
        let s = level_scale(obs.level_db);
        self.report.prediction_residual =
            prediction_residual(&self.current, obs.drives, obs.responses, &self.resid_band);
        if self.level != Some(obs.level_db) {
            self.level = Some(obs.level_db);
            self.pairs.clear();
        }
        self.pairs
            .push((obs.drives.to_vec(), obs.responses.to_vec()));
        self.nu2 = self.noise_variance(obs.responses);
        let (mut ysum, mut ycount) = (0.0, 0usize);
        for hat in &self.hats {
            for row in obs.responses {
                ysum += row[hat.line].norm_sqr();
                ycount += 1;
            }
        }
        let ysig2 = if ycount > 0 {
            ysum / ycount as f64
        } else {
            0.0
        };
        let start = self.cur.clone();
        let fo = self.fit(&start, ysig2);
        let np = 2 * self.nk;
        self.last_dof = Self::effective_dof(&fo.ata, &fo.q, np);
        self.last_costs = fo.costs;
        let (fitted, sig2) = (fo.x, fo.sig2);
        let keep = match self.opts.gate {
            None => true,
            Some(g) => {
                let gain = (self.misfit(&start) - self.misfit(&fitted)) / (2.0 * sig2);
                gain > g * self.last_dof / 2.0
            }
        };
        self.cur = if keep {
            fitted
        } else {
            self.refused += 1;
            start
        };
        self.store_put(s, self.cur.clone());
        let hw = self.model_at(&self.cur);
        self.report.fit_residual =
            prediction_residual(&hw, obs.drives, obs.responses, &self.resid_band);
        self.report.gate_fired = !keep;
        self.at_level = Some(self.refine(hw));
        Ok(())
    }

    fn model_for(&mut self, next_level_db: f64) -> Result<ModelSource, String> {
        let source = match self.level {
            None => {
                self.current = self.base.clone();
                ModelSource::Initial
            }
            Some(l) if l == next_level_db => {
                self.current = self
                    .at_level
                    .clone()
                    .unwrap_or_else(|| self.model_at(&self.cur));
                ModelSource::LevelOwn
            }
            Some(_) => {
                let xn = self.predict(level_scale(next_level_db));
                self.cur = xn;
                self.current = self.model_at(&self.cur);
                ModelSource::Predicted
            }
        };
        self.report.source = source;
        Ok(source)
    }

    fn report(&self) -> ModelReport {
        self.report
    }

    fn level_model(&self) -> Option<&MatrixStack> {
        self.at_level.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_unwrap_matches_the_usual_rule() {
        let mut p = vec![3.0, -3.0, 3.1, -3.0];
        unwrap(&mut p);
        let want = [3.0, -3.0 + TAU, 3.1, -3.0 + TAU];
        for (a, b) in p.iter().zip(want) {
            assert!((a - b).abs() < 1e-12, "{p:?}");
        }
    }

    #[test]
    fn mimo_shock_moving_average_is_centered_and_zero_padded() {
        let x = vec![1.0; 5];
        let m = moving_average(&x, 3);
        assert_eq!(m, vec![2.0 / 3.0, 1.0, 1.0, 1.0, 2.0 / 3.0]);
    }
}
