//! `followed_poles`: the carried inverse on a modal model whose poles follow
//! the structure through a sweep.
//!
//! The model is the identification's modes, found by [`crate::modal_fit`] with
//! the station's delay taken out, plus its smoothed remainder. Each update,
//! with `Z(f)` the regularized inverse of the model, `g` the loop-speed rule's
//! gain and `f'` the frequency of the next interval:
//!
//! ```text
//! D+ = D_f + w (Z(f') - Z(f)) H(f) D_f + Z(f') (R' - R) + g Z(f) (R - C)
//! ```
//!
//! - **The model in time.** The law runs its model at the update rate: each
//!   mode's resonant part as an envelope state, exact for one pole with the
//!   drive ramped over the interval, the rest quasi-static, the sum through
//!   the tracking filter's two one-pole stages at the present bandwidth. That
//!   is the model's own prediction of what the trackers read, build-up and
//!   filter lag included, so it can be set against the readings in a sweep.
//! - **Following the poles.** Every second update, over the last 40 updates,
//!   the frequency and damping of each mode within four bandwidths of the
//!   window's tones are refitted to the readings by two Gauss-Newton steps
//!   from where they are, with a prior that holds them to the
//!   identification's (four bandwidths in frequency, a factor `e` in damping).
//!   A mode moves only when one bandwidth of shift would change the window's
//!   prediction by more than the readings' scatter (taken as 2 % of the
//!   reference), at most a quarter bandwidth a fit and never more than 12
//!   bandwidths from where it was identified, its damping within 0.3 and 5
//!   times.
//! - **The gate.** The followed model's prediction residual over the window is
//!   set against the identification's own model run the same way. From the
//!   update the followed model is worse by twice, the inverse is the
//!   identification's, until it is better by twice.
//! - **The delay.** A modal fit with real residues cannot hold a pure delay,
//!   so the law is told the station's: the delay from the controller's
//!   writing a drive sample to its reading the response to it, the
//!   structure's own response left out (the controller's output queue, the
//!   output and acquisition chain, any transport delay). The identification
//!   a law is given has the controller's measured lag taken out already (its
//!   `transport_delay_s`, a cross-correlation's peak, which a structure's
//!   resonances move off the pure delay), so the fit removes the difference,
//!   `station_delay_s - transport_delay_s`, which may be negative, and puts it
//!   back on the residues. The law is refused without it.
//!
//! Sweeps only: at a dwell or a step the window holds one frequency and the
//! fit has nothing to follow.
//!
//! The law as the research study ran it, with two differences: the reference
//! of the next interval is aimed at the notch depth too (the study's was not;
//! the two agree when no notch is in), and the study's options that were off
//! by default (a complex scale per mode, a family of expected shifts as the
//! fallback) are not here.

use crate::law::{aimed_reference, worst_error_db, Hold, Inputs, Outputs, SineLaw};
use crate::linalg::{mat_vec, reg_pinv, solve_real};
use crate::modal::{q_mode, ModalForm, Model};
use crate::modal_fit::fit_known_delay;
use crate::setup::{GroupDelay, LoopDelay, Setup, TrackingRule};
use crate::speed::{gain, GainRule};
use crate::{TestTypes, C64};
use std::collections::VecDeque;
use std::f64::consts::TAU;

const ZERO: C64 = C64::new(0.0, 0.0);
const J: C64 = C64::new(0.0, 1.0);

/// The diagnostics `followed_poles` declares, in order.
pub const FOLLOWED_VALUES: [&str; 8] = [
    "gain",
    "error_db",
    "gated",
    "near_modes",
    "shift_bw",
    "damping_change",
    "prediction_ratio",
    "modes_kept",
];

/// The test types `followed_poles` accepts: sweeps only.
pub const FOLLOWED_TEST_TYPES: TestTypes = TestTypes {
    sweep: true,
    dwell: false,
    stepped: false,
};

/// Parameters of `followed_poles`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FollowedParams {
    /// The station's delay, s: between a drive and its reading, and not the
    /// structure's. Zero is not set, and the law is refused.
    pub station_delay_s: f64,
    /// The gain rule.
    pub gain_rule: GainRule,
    /// Regularization of the inverse, relative to the largest singular value.
    pub eps: f64,
    /// Weight on the model's change carried, 0 to 1.
    pub carry: f64,
    /// Half-width of the remainder's smoothing, lines.
    pub smoothing_lines: usize,
    /// Updates in the fit's window.
    pub window: usize,
    /// Updates between fits.
    pub fit_every: usize,
    /// Modes within this many bandwidths of the window's tones are refitted.
    pub near_bw: f64,
    /// The prior's spread on frequency, bandwidths.
    pub prior_bw: f64,
    /// The prior's spread on the logarithm of damping.
    pub prior_lnz: f64,
    /// The farthest a mode is followed, bandwidths.
    pub range_bw: f64,
    /// The most a mode moves in one fit, bandwidths.
    pub step_bw: f64,
    /// The readings' scatter, a share of the reference.
    pub data_rel: f64,
    /// A mode moves only when one bandwidth of shift changes the window's
    /// prediction by more than this many scatters.
    pub gate_snr: f64,
    /// Gauss-Newton steps a fit.
    pub iterations: usize,
    /// Fall back to the identification's inverse while the followed model
    /// predicts worse.
    pub gate: bool,
    /// The prediction ratio that switches the fallback on (its inverse, off).
    pub gate_ratio: f64,
}

impl Default for FollowedParams {
    fn default() -> Self {
        FollowedParams {
            station_delay_s: 0.0,
            gain_rule: GainRule::default(),
            eps: 1e-3,
            carry: 1.0,
            smoothing_lines: 8,
            window: 40,
            fit_every: 2,
            near_bw: 4.0,
            prior_bw: 4.0,
            prior_lnz: 1.0,
            range_bw: 12.0,
            step_bw: 0.25,
            data_rel: 0.02,
            gate_snr: 1.0,
            iterations: 2,
            gate: true,
            gate_ratio: 2.0,
        }
    }
}

/// One update the fit's window holds.
#[derive(Clone, Debug)]
struct Entry {
    f: f64,
    d0: Vec<C64>,
    d: Vec<C64>,
    c: Vec<C64>,
    ff: Vec<C64>,
    f0f: Vec<C64>,
    y: Vec<C64>,
    ystart: Vec<C64>,
    fr: Vec<f64>,
    zr: Vec<f64>,
}

/// The fit's window laid out for the prediction: per near mode, per update,
/// per control.
struct Window {
    f: Vec<f64>,
    adm: Vec<C64>,
    ade: Vec<C64>,
    qsr: Vec<C64>,
    yrun: Vec<C64>,
    a: Vec<f64>,
    ff: Vec<C64>,
    c: Vec<C64>,
    y0: Vec<C64>,
}

/// A mode's pole and its resonant part's residue factor.
fn pole(fr: f64, zr: f64) -> (C64, C64) {
    let wr = TAU * fr;
    let wd = wr * (1.0 - zr * zr).max(1e-6).sqrt();
    let p = C64::new(-zr * wr, wd);
    (p, p * p / (2.0 * J * wd))
}

/// `q = q_s + q_d`, with `q_d = k / (j w - p)` the resonant part.
fn q_split(f: f64, fr: f64, zr: f64) -> (C64, C64) {
    let w = J * (TAU * f);
    let (p, k) = pole(fr, zr);
    (1.0 + k.conj() / (w - p.conj()), k / (w - p))
}

/// `x_j = c_j x_(j-1) + b_j` over `j` from `x0` (`n` values a step), in the
/// study's form (a running sum of logarithms) so that the two round alike.
fn scan(c: &[C64], b: &[C64], x0: &[C64], n: usize, out: &mut [C64]) {
    let mut l = ZERO;
    let mut acc = vec![ZERO; n];
    for (j, cj) in c.iter().enumerate() {
        l += cj.ln();
        let (up, down) = (l.exp(), (-l).exp());
        for i in 0..n {
            acc[i] += b[j * n + i] * down;
            out[j * n + i] = up * (x0[i] + acc[i]);
        }
    }
}

/// [`scan`] with real factors and `x0 = 0`, for the filter stages.
fn scan_real(c: &[f64], b: &[C64], n: usize, out: &mut [C64]) {
    let mut l = 0.0f64;
    let mut acc = vec![ZERO; n];
    for (j, cj) in c.iter().enumerate() {
        l += cj.ln();
        let (up, down) = (l.exp(), (-l).exp());
        for i in 0..n {
            acc[i] += b[j * n + i] * down;
            out[j * n + i] = acc[i] * up;
        }
    }
}

/// The `followed_poles` law's state.
#[derive(Clone, Debug)]
pub struct FollowedLaw {
    p: FollowedParams,
    n: usize,
    m: usize,
    tick_s: f64,
    tracking: TrackingRule,
    loop_delay: LoopDelay,
    delay: GroupDelay,
    grid_lo: f64,
    grid_hi: f64,
    setup: Setup,
    model: Model,
    fa: Vec<f64>,
    za: Vec<f64>,
    fr: Vec<f64>,
    zr: Vec<f64>,
    y: Vec<C64>,
    y0: Vec<C64>,
    filt: [Vec<C64>; 2],
    filt0: [Vec<C64>; 2],
    hist: VecDeque<Entry>,
    y_last: Option<Vec<C64>>,
    gated: bool,
    d_prev: Vec<C64>,
    k: u64,
}

impl FollowedLaw {
    /// Build the law: the modal fit is made here, once, with the station's
    /// delay less the transport delay taken out.
    pub fn new(setup: &Setup, params: FollowedParams) -> Result<FollowedLaw, String> {
        setup.check()?;
        Self::check_params(&params)?;
        let tau = params.station_delay_s - setup.loop_delay.transport_delay_s;
        let modes = fit_known_delay(
            &setup.identification,
            (setup.band_lo_hz, setup.band_hi_hz),
            tau,
        )?;
        Self::with_modes(setup, params, modes)
    }

    /// Build the law on modes already found (the station's delay must still
    /// be set, though it is not read).
    pub fn with_modes(
        setup: &Setup,
        params: FollowedParams,
        modes: ModalForm,
    ) -> Result<FollowedLaw, String> {
        setup.check()?;
        Self::check_params(&params)?;
        let id = &setup.identification;
        let model = Model::from_modes(id, modes, params.smoothing_lines)?;
        let (n, m) = (setup.controls, setup.drives);
        let mm = model.modes.modes();
        let (fa, za) = (model.modes.f_hz.clone(), model.modes.zeta.clone());
        Ok(FollowedLaw {
            p: params,
            n,
            m,
            tick_s: setup.tick_s,
            tracking: setup.tracking,
            loop_delay: setup.loop_delay,
            delay: GroupDelay::of_with(id, params.gain_rule.delay_estimate()),
            grid_lo: id.grid_hz[0],
            grid_hi: id.grid_hz[id.grid_hz.len() - 1],
            setup: setup.clone(),
            fr: fa.clone(),
            zr: za.clone(),
            fa,
            za,
            model,
            y: vec![ZERO; mm * n],
            y0: vec![ZERO; mm * n],
            filt: [vec![ZERO; n], vec![ZERO; n]],
            filt0: [vec![ZERO; n], vec![ZERO; n]],
            hist: VecDeque::new(),
            y_last: None,
            gated: false,
            d_prev: vec![ZERO; m],
            k: 0,
        })
    }

    fn check_params(p: &FollowedParams) -> Result<(), String> {
        if !(p.station_delay_s.is_finite() && p.station_delay_s > 0.0) {
            return Err(
                "followed_poles needs the station's delay (station_delay_s): the delay \
                        between a drive and its reading that is not the structure's, as a \
                        loopback identification measures it"
                    .into(),
            );
        }
        if !(0.0..=1.0).contains(&p.carry) {
            return Err("the carry weight must be 0 to 1".into());
        }
        if p.window < 2 || p.fit_every < 1 || p.iterations < 1 {
            return Err("the window needs two updates and the fit a step".into());
        }
        let positive = [
            p.near_bw,
            p.prior_bw,
            p.prior_lnz,
            p.range_bw,
            p.step_bw,
            p.data_rel,
            p.gate_ratio,
        ];
        if positive.iter().any(|v| !(v.is_finite() && *v > 0.0))
            || !(p.gate_snr.is_finite() && p.gate_snr >= 0.0)
            || !(p.eps.is_finite() && p.eps >= 0.0)
        {
            return Err("the followed-poles settings must be positive numbers".into());
        }
        Ok(())
    }

    /// The model the law reads (its modes and its smoothed grid).
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// The followed frequencies and dampings now.
    pub fn followed(&self) -> (&[f64], &[f64]) {
        (&self.fr, &self.zr)
    }

    /// Whether the identification's inverse is in use.
    pub fn gated(&self) -> bool {
        self.gated
    }

    fn modes(&self) -> usize {
        self.fa.len()
    }

    fn residue(&self, r: usize) -> &[C64] {
        let e = self.n * self.m;
        &self.model.modes.residues[r * e..(r + 1) * e]
    }

    fn base_at(&self, f: f64) -> Vec<C64> {
        let mut h = vec![ZERO; self.n * self.m];
        self.model.at(f, &mut h);
        h
    }

    /// The base plus `sum_r d_r A_r` (the sum taken first, as the study did).
    fn base_plus(&self, f: f64, d: &[C64]) -> Vec<C64> {
        let mut s = vec![ZERO; self.n * self.m];
        for (r, dr) in d.iter().enumerate() {
            for (sv, a) in s.iter_mut().zip(self.residue(r)) {
                *sv += dr * a;
            }
        }
        let mut h = self.base_at(f);
        h.iter_mut().zip(&s).for_each(|(hv, sv)| *hv += sv);
        h
    }

    /// The followed model at `f`.
    fn h_at(&self, f: f64) -> Vec<C64> {
        let d: Vec<C64> = (0..self.modes())
            .map(|r| q_mode(f, self.fr[r], self.zr[r]) - q_mode(f, self.fa[r], self.za[r]))
            .collect();
        self.base_plus(f, &d)
    }

    fn z_at(&self, f: f64) -> Result<Vec<C64>, String> {
        let h = if self.gated {
            self.base_at(f)
        } else {
            self.h_at(f)
        };
        Ok(reg_pinv(&h, self.n, self.m, self.p.eps)?.0)
    }

    fn alpha(&self, f: f64) -> f64 {
        1.0 - (-TAU * self.tracking.bandwidth_hz(f) * self.tick_s).exp()
    }

    /// `A_r d`, per control.
    fn ad(&self, r: usize, d: &[C64]) -> Vec<C64> {
        let mut out = vec![ZERO; self.n];
        mat_vec(self.residue(r), self.n, self.m, d, &mut out);
        out
    }

    /// One interval of the envelope states with the drive ramped `d0 -> d1`.
    fn env_step(&self, y: &mut [C64], f: f64, d0: &[C64], d1: &[C64], fr: &[f64], zr: &[f64]) {
        let mid: Vec<C64> = d0.iter().zip(d1).map(|(a, b)| 0.5 * (a + b)).collect();
        let n = self.n;
        for r in 0..self.modes() {
            let (p, kk) = pole(fr[r], zr[r]);
            let lam = p - J * (TAU * f);
            let e = (lam * self.tick_s).exp();
            let w = (e - 1.0) / lam * kk;
            let ad = self.ad(r, &mid);
            for i in 0..n {
                y[r * n + i] = e * y[r * n + i] + w * ad[i];
            }
        }
    }

    /// The model's quasi-static part times `d`, poles at `fr`, `zr`.
    fn quasi(&self, f: f64, d: &[C64], fr: &[f64], zr: &[f64]) -> Vec<C64> {
        let dd: Vec<C64> = (0..self.modes())
            .map(|r| {
                let (qs0, qd0) = q_split(f, self.fa[r], self.za[r]);
                q_split(f, fr[r], zr[r]).0 - qs0 - qd0
            })
            .collect();
        let h = self.base_plus(f, &dd);
        let mut out = vec![ZERO; self.n];
        mat_vec(&h, self.n, self.m, d, &mut out);
        out
    }

    fn filter(&self, st: &mut [Vec<C64>; 2], p: &[C64], f: f64) {
        let a = self.alpha(f);
        for i in 0..self.n {
            let f1 = st[0][i] + a * (p[i] - st[0][i]);
            let f2 = st[1][i] + a * (f1 - st[1][i]);
            st[0][i] = f1;
            st[1][i] = f2;
        }
    }

    /// The sum over modes of the envelope states, per control.
    fn envelope_sum(&self, y: &[C64]) -> Vec<C64> {
        let n = self.n;
        (0..n)
            .map(|i| (0..self.modes()).map(|r| y[r * n + i]).sum())
            .collect()
    }

    /// Run the followed model and the identification's over the interval just
    /// played.
    fn advance(&mut self, x: &Inputs) {
        let (d0, d1) = (self.d_prev.clone(), x.drive_in_force);
        let (mut y, mut y0) = (std::mem::take(&mut self.y), std::mem::take(&mut self.y0));
        self.env_step(&mut y, x.f_hz, &d0, d1, &self.fr, &self.zr);
        self.env_step(&mut y0, x.f_hz, &d0, d1, &self.fa, &self.za);
        let (s, s0) = (self.envelope_sum(&y), self.envelope_sum(&y0));
        let mut p = self.quasi(x.f_hz, d1, &self.fr, &self.zr);
        let mut p0 = self.quasi(x.f_hz, d1, &self.fa, &self.za);
        p.iter_mut().zip(&s).for_each(|(a, b)| *a += b);
        p0.iter_mut().zip(&s0).for_each(|(a, b)| *a += b);
        self.y = y;
        self.y0 = y0;
        let (mut st, mut st0) = (self.filt.clone(), self.filt0.clone());
        self.filter(&mut st, &p, x.f_hz);
        self.filter(&mut st0, &p0, x.f_hz);
        self.filt = st;
        self.filt0 = st0;
    }

    fn window(&self, idx: &[usize]) -> Window {
        let (n, nw) = (self.n, self.hist.len());
        let k = idx.len();
        let mut w = Window {
            f: self.hist.iter().map(|e| e.f).collect(),
            adm: vec![ZERO; k * nw * n],
            ade: vec![ZERO; k * nw * n],
            qsr: vec![ZERO; k * nw],
            yrun: vec![ZERO; k * nw * n],
            a: Vec::with_capacity(nw),
            ff: Vec::with_capacity(nw * n),
            c: Vec::with_capacity(nw * n),
            y0: Vec::with_capacity(k * n),
        };
        for (j, e) in self.hist.iter().enumerate() {
            let mid: Vec<C64> = e.d0.iter().zip(&e.d).map(|(a, b)| 0.5 * (a + b)).collect();
            for (ki, &r) in idx.iter().enumerate() {
                let o = (ki * nw + j) * n;
                w.adm[o..o + n].copy_from_slice(&self.ad(r, &mid));
                w.ade[o..o + n].copy_from_slice(&self.ad(r, &e.d));
                w.qsr[ki * nw + j] = q_split(e.f, e.fr[r], e.zr[r]).0;
                w.yrun[o..o + n].copy_from_slice(&e.y[r * n..(r + 1) * n]);
            }
            w.a.push(self.alpha(e.f));
            w.ff.extend_from_slice(&e.ff);
            w.c.extend_from_slice(&e.c);
        }
        let first = &self.hist[0].ystart;
        for &r in idx {
            w.y0.extend_from_slice(&first[r * n..(r + 1) * n]);
        }
        w
    }

    /// The window's predicted readings (`W x n`) with the near modes at
    /// `fr`, `zr` and every other mode as it ran. The candidates' envelope
    /// states start from the states kept at the window's start; the change
    /// they make passes the tracking filter from zero there.
    fn predict(&self, w: &Window, fr: &[f64], zr: &[f64]) -> Vec<C64> {
        let (n, nw, dt) = (self.n, w.f.len(), self.tick_s);
        let mut quasi = vec![ZERO; nw * n];
        let mut env = vec![ZERO; nw * n];
        let mut ys = vec![ZERO; nw * n];
        for ki in 0..fr.len() {
            let (p, kk) = pole(fr[ki], zr[ki]);
            let mut c = Vec::with_capacity(nw);
            let mut b = vec![ZERO; nw * n];
            for (j, &f) in w.f.iter().enumerate() {
                let lam = p - J * (TAU * f);
                let ex = (lam * dt).exp();
                let s = (ex - 1.0) / lam * kk;
                for i in 0..n {
                    b[j * n + i] = s * w.adm[(ki * nw + j) * n + i];
                }
                c.push(ex);
            }
            scan(&c, &b, &w.y0[ki * n..(ki + 1) * n], n, &mut ys);
            for (j, &f) in w.f.iter().enumerate() {
                let dq = q_split(f, fr[ki], zr[ki]).0 - w.qsr[ki * nw + j];
                for i in 0..n {
                    let (o, s) = (j * n + i, (ki * nw + j) * n + i);
                    quasi[o] += dq * w.ade[s];
                    env[o] += ys[o] - w.yrun[s];
                }
            }
        }
        let c: Vec<f64> = w.a.iter().map(|a| 1.0 - a).collect();
        let mut b: Vec<C64> = (0..nw * n)
            .map(|o| (quasi[o] + env[o]) * w.a[o / n])
            .collect();
        let mut x1 = vec![ZERO; nw * n];
        scan_real(&c, &b, n, &mut x1);
        for (o, v) in b.iter_mut().enumerate() {
            *v = x1[o] * w.a[o / n];
        }
        let mut x2 = vec![ZERO; nw * n];
        scan_real(&c, &b, n, &mut x2);
        w.ff.iter().zip(&x2).map(|(a, b)| a + b).collect()
    }

    /// The fit's residual at `t`: the near modes' frequency shifts in
    /// anchor bandwidths, then their log damping ratios.
    fn residual(&self, w: &Window, idx: &[usize], bw0: &[f64], sd: f64, t: &[f64]) -> Vec<f64> {
        let k = idx.len();
        let fr: Vec<f64> = (0..k).map(|i| self.fa[idx[i]] + t[i] * bw0[i]).collect();
        let zr: Vec<f64> = (0..k).map(|i| self.za[idx[i]] * t[k + i].exp()).collect();
        let pred = self.predict(w, &fr, &zr);
        let r: Vec<C64> = w.c.iter().zip(&pred).map(|(c, p)| (c - p) / sd).collect();
        let mut out: Vec<f64> = r.iter().map(|v| v.re).collect();
        out.extend(r.iter().map(|v| v.im));
        out.extend((0..k).map(|i| t[i] / self.p.prior_bw));
        out.extend((0..k).map(|i| t[k + i] / self.p.prior_lnz));
        out
    }

    /// Refit the near modes' poles to the window.
    fn fit(&mut self, reference: &[C64]) {
        let mm = self.modes();
        if mm == 0 || self.hist.len() < self.p.window {
            return;
        }
        let idx: Vec<usize> = (0..mm)
            .filter(|&r| {
                let bw = 2.0 * self.zr[r] * self.fr[r];
                let d = self
                    .hist
                    .iter()
                    .map(|e| (e.f - self.fr[r]).abs())
                    .fold(f64::INFINITY, f64::min);
                d / bw < self.p.near_bw
            })
            .collect();
        if idx.is_empty() {
            return;
        }
        let w = self.window(&idx);
        let rn = reference.iter().map(|v| v.norm_sqr()).sum::<f64>().sqrt();
        let sd = self.p.data_rel * rn.max(1e-12);
        let k = idx.len();
        let npar = 2 * k;
        let bw0: Vec<f64> = idx.iter().map(|&r| 2.0 * self.za[r] * self.fa[r]).collect();
        let mut t: Vec<f64> = idx
            .iter()
            .zip(&bw0)
            .map(|(&r, b)| (self.fr[r] - self.fa[r]) / b)
            .chain(idx.iter().map(|&r| (self.zr[r] / self.za[r]).ln()))
            .collect();
        let nd = 2 * w.c.len();
        let hstep: Vec<f64> = (0..npar).map(|i| if i < k { 0.05 } else { 0.02 }).collect();
        let sq = |v: &[f64]| v.iter().map(|x| x * x).sum::<f64>();
        for _ in 0..self.p.iterations {
            let r0 = self.residual(&w, &idx, &bw0, sd, &t);
            let nres = r0.len();
            // The Jacobian by forward differences.
            let mut jm = vec![0.0f64; nres * npar];
            for c in 0..npar {
                let mut tc = t.clone();
                tc[c] += hstep[c];
                let rc = self.residual(&w, &idx, &bw0, sd, &tc);
                for i in 0..nres {
                    jm[i * npar + c] = (rc[i] - r0[i]) / hstep[c];
                }
            }
            // The data gate: one bandwidth's effect on the window's prediction.
            let upd: Vec<usize> = (0..k)
                .filter(|&c| {
                    let s: f64 = (0..nd).map(|i| jm[i * npar + c].powi(2)).sum();
                    s.sqrt() > self.p.gate_snr
                })
                .collect();
            if upd.is_empty() {
                break;
            }
            let cols: Vec<usize> = upd
                .iter()
                .copied()
                .chain(upd.iter().map(|u| k + u))
                .collect();
            let nc = cols.len();
            let mut jtj = vec![0.0f64; nc * nc];
            let mut rhs = vec![0.0f64; nc];
            for (a, &ca) in cols.iter().enumerate() {
                for (b, &cb) in cols.iter().enumerate() {
                    jtj[a * nc + b] = (0..nres)
                        .map(|i| jm[i * npar + ca] * jm[i * npar + cb])
                        .sum();
                }
                jtj[a * nc + a] += 1e-6;
                rhs[a] = -(0..nres).map(|i| jm[i * npar + ca] * r0[i]).sum::<f64>();
            }
            let Some(stp) = solve_real(&jtj, &rhs, nc) else {
                break;
            };
            let mut full = vec![0.0f64; npar];
            for (a, &c) in cols.iter().enumerate() {
                full[c] = stp[a];
            }
            let mut tn = t.clone();
            for i in 0..k {
                tn[i] = (t[i] + full[i].clamp(-self.p.step_bw, self.p.step_bw))
                    .clamp(-self.p.range_bw, self.p.range_bw);
                tn[k + i] =
                    (t[k + i] + full[k + i].clamp(-0.2, 0.2)).clamp(0.3f64.ln(), 5.0f64.ln());
            }
            if sq(&self.residual(&w, &idx, &bw0, sd, &tn)) < sq(&r0) {
                t = tn;
            } else {
                break;
            }
        }
        for (i, &r) in idx.iter().enumerate() {
            self.fr[r] = self.fa[r] + t[i] * bw0[i];
            self.zr[r] = self.za[r] * t[k + i].exp();
        }
    }

    fn engage_impl(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let r = aimed_reference(x);
        let z = self.z_at(x.f_hz)?;
        mat_vec(&z, self.m, self.n, &r, &mut out.drive);
        self.d_prev.iter_mut().for_each(|v| *v = ZERO);
        out.fault = false;
        out.values.clear();
        out.values
            .extend_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -1.0, self.modes() as f64]);
        Ok(())
    }

    fn tick_impl(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        let (n, m) = (self.n, self.m);
        self.k += 1;
        self.advance(x);
        // The state at the start of this interval is what a window beginning
        // here runs from.
        let ystart = self.y_last.take().unwrap_or_else(|| self.y.clone());
        self.y_last = Some(self.y.clone());
        let entry = Entry {
            f: x.f_hz,
            d0: self.d_prev.clone(),
            d: x.drive_in_force.to_vec(),
            c: x.controls.to_vec(),
            ff: self.filt[1].clone(),
            f0f: self.filt0[1].clone(),
            y: self.y.clone(),
            ystart,
            fr: self.fr.clone(),
            zr: self.zr.clone(),
        };
        self.d_prev.copy_from_slice(x.drive_in_force);
        if x.hold != Hold::Engage {
            self.hist.push_back(entry);
            if self.hist.len() > self.p.window {
                self.hist.pop_front();
            }
        }
        let r = aimed_reference(x);
        let mut rho = -1.0;
        if self.hist.len() >= self.p.window {
            if self.k.is_multiple_of(self.p.fit_every as u64) {
                self.fit(&r);
            }
            let (mut rf, mut r0) = (0.0, 0.0);
            for e in &self.hist {
                for i in 0..n {
                    rf += (e.c[i] - e.ff[i]).norm_sqr();
                    r0 += (e.c[i] - e.f0f[i]).norm_sqr();
                }
            }
            let v = rf / r0.max(1e-300);
            if v.is_finite() {
                rho = v;
                if self.p.gate {
                    if !self.gated && v > self.p.gate_ratio {
                        self.gated = true;
                    } else if self.gated && v < 1.0 / self.p.gate_ratio {
                        self.gated = false;
                    }
                }
            }
        }
        // The carried inverse.
        let gn = gain(
            self.p.gain_rule,
            &self.tracking,
            self.loop_delay,
            &self.delay,
            self.tick_s,
            x.f_hz,
        );
        let g = if x.hold == Hold::Engage { 0.0 } else { gn.g };
        let notch = if x.notch_scale.is_finite() {
            x.notch_scale.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let f_next = (x.f_hz + x.sweep_hz_s * self.tick_s).clamp(self.grid_lo, self.grid_hi);
        let mut rn = vec![ZERO; n];
        self.setup.reference_at(f_next, &mut rn);
        rn.iter_mut().for_each(|v| *v = *v * x.level * notch);
        let hk = self.h_at(x.f_hz);
        let zk = self.z_at(x.f_hz)?;
        let zn = self.z_at(f_next)?;
        let d0 = x.drive_in_force;
        let mut c0 = vec![ZERO; n];
        mat_vec(&hk, n, m, d0, &mut c0);
        let dz: Vec<C64> = zn.iter().zip(&zk).map(|(a, b)| a - b).collect();
        let mut carry = vec![ZERO; m];
        mat_vec(&dz, m, n, &c0, &mut carry);
        let dr: Vec<C64> = rn.iter().zip(&r).map(|(p, q)| p - q).collect();
        let mut moved = vec![ZERO; m];
        mat_vec(&zn, m, n, &dr, &mut moved);
        let err: Vec<C64> = r.iter().zip(x.controls).map(|(p, q)| p - q).collect();
        let mut corr = vec![ZERO; m];
        mat_vec(&zk, m, n, &err, &mut corr);
        for i in 0..m {
            out.drive[i] = d0[i] + self.p.carry * carry[i] + moved[i] + g * corr[i];
        }
        // What the record shows of the follower near the tone.
        let (mut near, mut shift, mut lnz) = (0usize, 0.0f64, 0.0f64);
        for i in 0..self.modes() {
            let bw = 2.0 * self.za[i] * self.fa[i];
            if (x.f_hz - self.fr[i]).abs() / bw < self.p.near_bw {
                near += 1;
                shift = shift.max((self.fr[i] - self.fa[i]).abs() / bw);
                lnz = lnz.max((self.zr[i] / self.za[i]).ln().abs());
            }
        }
        out.fault = false;
        out.values.clear();
        out.values.extend_from_slice(&[
            g,
            worst_error_db(x.controls, &r),
            if self.gated { 1.0 } else { 0.0 },
            near as f64,
            shift,
            lnz,
            rho,
            self.modes() as f64,
        ]);
        Ok(())
    }
}

impl SineLaw for FollowedLaw {
    fn engage(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.engage_impl(x, out)
    }
    fn tick(&mut self, x: &Inputs, out: &mut Outputs) -> Result<(), String> {
        self.tick_impl(x, out)
    }
    fn value_names(&self) -> &'static [&'static str] {
        &FOLLOWED_VALUES
    }
}
