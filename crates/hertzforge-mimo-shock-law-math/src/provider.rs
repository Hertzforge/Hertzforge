//! The model seam: how a law's model follows the plant, kept apart from the
//! law's correction (`Derivations.md` D9).
//!
//! A law asks its [`ModelProvider`] for the model for the level it is about to
//! play ([`ModelProvider::model_for`]) and tells it each pulse played
//! ([`ModelProvider::observe`]), as full-level spectra. Providers:
//!
//! - [`FrozenModel`]: the identification, never changed (today's default).
//! - [`RefreshModel`]: the rank-one move along each pulse's drive
//!   ([`crate::refresh`], today's option).
//! - [`MultiSecantModel`]: every pulse at the current level is a constraint;
//!   the model is the least change from the level's starting model that
//!   satisfies all of them, with a gate that keeps only pairs whose drive adds
//!   a new direction (`Derivations.md` D14).
//! - [`PoleTrackModel`]: a modal model whose modes' frequencies and damping are
//!   re-fitted from every pulse by Gauss-Newton, extrapolated in level for the
//!   first pulse of a new level, with a validity gate that hands over to a
//!   fallback provider when the tracked model predicts the pulses worse than
//!   the fallback does; options: a residue re-fit, a sparse (lasso or group
//!   lasso) penalty on the per-pulse changes, and a same-level secant on top of
//!   the tracked model (`Derivations.md` D11 to D13).

use crate::lsq::{hpd_solve, spd_solve, spectral_norm};
use crate::modal::{modal_q, modal_q_derivs, ModalModel};
use crate::refresh::refresh_along_drive;
use crate::{level_scale, MatrixStack, C64};
use std::f64::consts::TAU;

/// One pulse as a provider sees it: full-level spectra on every line.
#[derive(Clone, Copy, Debug)]
pub struct Observation<'a> {
    /// Level the pulse was played at, dB.
    pub level_db: f64,
    /// Drive spectra, `n_d` rows of lines, divided by the pulse's scale.
    pub drives: &'a [Vec<C64>],
    /// Response spectra, `n_c` rows of lines, divided by the pulse's scale.
    pub responses: &'a [Vec<C64>],
}

/// Where the model for the next pulse came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelSource {
    /// The identification as given.
    Initial,
    /// Learned at the level about to be played.
    LevelOwn,
    /// Predicted for a level not yet played.
    Predicted,
    /// The tracked model was refused; the fallback provider's model.
    Fallback,
}

impl ModelSource {
    /// A number for diagnostics: 0 initial, 1 level's own, 2 predicted, 3
    /// fallback.
    pub fn code(self) -> f64 {
        match self {
            ModelSource::Initial => 0.0,
            ModelSource::LevelOwn => 1.0,
            ModelSource::Predicted => 2.0,
            ModelSource::Fallback => 3.0,
        }
    }
}

/// What a provider reports after a pulse. NaN where it does not apply.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelReport {
    /// `||Y - Hh D|| / ||Y||` over the provider's band, of the model that
    /// played the pulse.
    pub prediction_residual: f64,
    /// The same after the provider's update on that pulse.
    pub fit_residual: f64,
    /// The fallback's prediction residual on the pulse.
    pub fallback_residual: f64,
    /// Whether the validity gate handed the next pulse to the fallback.
    pub gate_fired: bool,
    /// Where the model for the next pulse came from.
    pub source: ModelSource,
}

impl Default for ModelReport {
    fn default() -> Self {
        ModelReport {
            prediction_residual: f64::NAN,
            fit_residual: f64::NAN,
            fallback_residual: f64::NAN,
            gate_fired: false,
            source: ModelSource::Initial,
        }
    }
}

/// How a law's model follows the plant (module documentation).
pub trait ModelProvider: Send + Sync {
    /// The model now in force.
    fn current(&self) -> &MatrixStack;
    /// Learns from a pulse just played.
    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String>;
    /// Makes the model for the next pulse, at `next_level_db`, current.
    /// Returns where it came from.
    fn model_for(&mut self, next_level_db: f64) -> Result<ModelSource, String>;
    /// The report on the last pulse.
    fn report(&self) -> ModelReport;
    /// Per line, the largest spectral-norm change of the model for
    /// `next_level_db` when every mode's frequency is lowered by each of
    /// `drops` (fractions). Only a provider with a modal model has one.
    fn radius(&self, _next_level_db: f64, _drops: &[f64]) -> Option<Vec<f64>> {
        None
    }
    /// The model as learned at the level of the last pulse observed (before
    /// any prediction for another level), when the provider has one apart from
    /// [`ModelProvider::current`].
    fn level_model(&self) -> Option<&MatrixStack> {
        None
    }
}

/// `||Y - H D|| / ||Y||` over the flagged lines.
pub fn prediction_residual(
    model: &MatrixStack,
    drives: &[Vec<C64>],
    responses: &[Vec<C64>],
    band: &[bool],
) -> f64 {
    let (_, n_c, n_d) = model.dims();
    let (mut num, mut den) = (0.0, 0.0);
    for (f, &on) in band.iter().enumerate() {
        if !on {
            continue;
        }
        let line = model.line(f);
        for r in 0..n_c {
            let mut hd = C64::new(0.0, 0.0);
            for c in 0..n_d {
                hd += line[r * n_d + c] * drives[c][f];
            }
            num += (responses[r][f] - hd).norm_sqr();
            den += responses[r][f].norm_sqr();
        }
    }
    if den > 0.0 {
        (num / den).sqrt()
    } else {
        f64::NAN
    }
}

/// The identification, never changed.
pub struct FrozenModel {
    model: MatrixStack,
    band: Vec<bool>,
    report: ModelReport,
}

impl FrozenModel {
    /// A frozen provider; `band` is where its residual is measured.
    pub fn new(model: MatrixStack, band: Vec<bool>) -> Self {
        FrozenModel {
            model,
            band,
            report: ModelReport::default(),
        }
    }
}

impl ModelProvider for FrozenModel {
    fn current(&self) -> &MatrixStack {
        &self.model
    }
    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String> {
        let r = prediction_residual(&self.model, obs.drives, obs.responses, &self.band);
        self.report.prediction_residual = r;
        self.report.fit_residual = r;
        Ok(())
    }
    fn model_for(&mut self, _next_level_db: f64) -> Result<ModelSource, String> {
        Ok(ModelSource::Initial)
    }
    fn report(&self) -> ModelReport {
        self.report
    }
}

/// The rank-one refresh along each pulse's drive.
pub struct RefreshModel {
    model: MatrixStack,
    band: Vec<bool>,
    mu: f64,
    floor_rel: f64,
    report: ModelReport,
}

impl RefreshModel {
    /// A refresh provider with blend `mu` and drive-energy floor `floor_rel`.
    pub fn new(model: MatrixStack, band: Vec<bool>, mu: f64, floor_rel: f64) -> Self {
        RefreshModel {
            model,
            band,
            mu,
            floor_rel,
            report: ModelReport::default(),
        }
    }
}

impl ModelProvider for RefreshModel {
    fn current(&self) -> &MatrixStack {
        &self.model
    }
    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String> {
        self.report.prediction_residual =
            prediction_residual(&self.model, obs.drives, obs.responses, &self.band);
        refresh_along_drive(
            &mut self.model,
            obs.drives,
            obs.responses,
            self.mu,
            self.floor_rel,
        )?;
        self.report.fit_residual =
            prediction_residual(&self.model, obs.drives, obs.responses, &self.band);
        self.report.source = ModelSource::LevelOwn;
        Ok(())
    }
    fn model_for(&mut self, _next_level_db: f64) -> Result<ModelSource, String> {
        Ok(self.report.source)
    }
    fn report(&self) -> ModelReport {
        self.report
    }
}

/// Options of the same-level multi-secant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SecantOptions {
    /// A pair is used only if the part of its drive orthogonal to the drives
    /// of the newer pairs already used is at least this fraction of it, per
    /// line. Zero uses every pair (the ungated form).
    pub gate: f64,
    /// Lines whose newest drive energy is below this fraction of the largest
    /// line's keep the level's starting model.
    pub floor_rel: f64,
    /// Regularization of the pairs' Gram matrix, relative to its trace.
    pub eps_rel: f64,
}

impl Default for SecantOptions {
    fn default() -> Self {
        SecantOptions {
            gate: 0.0,
            floor_rel: 1e-12,
            eps_rel: 1e-10,
        }
    }
}

type Pair = (Vec<Vec<C64>>, Vec<Vec<C64>>);

/// The least change of `prior` that reproduces every stored pair on every
/// line, the pairs gated per line (newest first) by `opts.gate`.
pub fn secant_correct(prior: &MatrixStack, pairs: &[Pair], opts: &SecantOptions) -> MatrixStack {
    let mut h = prior.clone();
    if pairs.is_empty() {
        return h;
    }
    let (lines, n_c, n_d) = prior.dims();
    let newest = &pairs[pairs.len() - 1].0;
    let energy: Vec<f64> = (0..lines)
        .map(|f| newest.iter().map(|d| d[f].norm_sqr()).sum())
        .collect();
    let peak = energy.iter().fold(0.0f64, |a, &b| a.max(b));
    if peak <= 0.0 {
        return h;
    }
    let zero = C64::new(0.0, 0.0);
    for (f, &ef) in energy.iter().enumerate() {
        if ef <= opts.floor_rel * peak {
            continue;
        }
        // Gate: newest first, keep a pair only if it adds a direction.
        let mut basis: Vec<Vec<C64>> = Vec::new();
        let mut used: Vec<usize> = Vec::new();
        for i in (0..pairs.len()).rev() {
            let v: Vec<C64> = (0..n_d).map(|c| pairs[i].0[c][f]).collect();
            let nv: f64 = v.iter().map(|x| x.norm_sqr()).sum::<f64>().sqrt();
            if nv == 0.0 {
                continue;
            }
            let mut p = v.clone();
            for b in &basis {
                let dot: C64 = b.iter().zip(&v).map(|(x, y)| x.conj() * y).sum();
                for (pc, bc) in p.iter_mut().zip(b) {
                    *pc -= dot * bc;
                }
            }
            let np: f64 = p.iter().map(|x| x.norm_sqr()).sum::<f64>().sqrt();
            if used.is_empty() || np >= opts.gate * nv {
                used.push(i);
                if np > 1e-14 * nv {
                    basis.push(p.iter().map(|x| x / np).collect());
                }
            }
            if opts.gate > 0.0 && used.len() >= n_d {
                break;
            }
        }
        let m = used.len();
        // Gram G = Dm^H Dm (m x m), residual E = Ym - Hp Dm (n_c x m).
        let mut g = vec![zero; m * m];
        for (a, &ia) in used.iter().enumerate() {
            for (b, &ib) in used.iter().enumerate() {
                g[a * m + b] = (0..n_d)
                    .map(|c| pairs[ia].0[c][f].conj() * pairs[ib].0[c][f])
                    .sum();
            }
        }
        let tr: f64 = (0..m).map(|a| g[a * m + a].re).sum();
        for a in 0..m {
            g[a * m + a] += C64::new(opts.eps_rel * tr, 0.0);
        }
        // X = G^-1 Dm^H (m x n_d).
        let mut rhs = vec![zero; m * n_d];
        for (a, &ia) in used.iter().enumerate() {
            for c in 0..n_d {
                rhs[a * n_d + c] = pairs[ia].0[c][f].conj();
            }
        }
        let x = match hpd_solve(&g, m, &rhs, n_d) {
            Ok(x) => x,
            Err(_) => continue,
        };
        let line = h.line_mut(f);
        let pl = prior.line(f);
        for r in 0..n_c {
            let e: Vec<C64> = used
                .iter()
                .map(|&i| {
                    let mut hd = zero;
                    for c in 0..n_d {
                        hd += pl[r * n_d + c] * pairs[i].0[c][f];
                    }
                    pairs[i].1[r][f] - hd
                })
                .collect();
            for c in 0..n_d {
                let mut acc = zero;
                for a in 0..m {
                    acc += e[a] * x[a * n_d + c];
                }
                line[r * n_d + c] += acc;
            }
        }
    }
    h
}

/// The same-level multi-secant provider.
pub struct MultiSecantModel {
    model: MatrixStack,
    prior: MatrixStack,
    band: Vec<bool>,
    level: Option<f64>,
    pairs: Vec<Pair>,
    opts: SecantOptions,
    report: ModelReport,
}

impl MultiSecantModel {
    /// A multi-secant provider starting from `model`.
    pub fn new(model: MatrixStack, band: Vec<bool>, opts: SecantOptions) -> Self {
        MultiSecantModel {
            prior: model.clone(),
            model,
            band,
            level: None,
            pairs: Vec::new(),
            opts,
            report: ModelReport::default(),
        }
    }
}

impl ModelProvider for MultiSecantModel {
    fn current(&self) -> &MatrixStack {
        &self.model
    }
    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String> {
        self.report.prediction_residual =
            prediction_residual(&self.model, obs.drives, obs.responses, &self.band);
        if self.level != Some(obs.level_db) {
            self.prior = self.model.clone();
            self.pairs.clear();
            self.level = Some(obs.level_db);
        }
        self.pairs
            .push((obs.drives.to_vec(), obs.responses.to_vec()));
        self.model = secant_correct(&self.prior, &self.pairs, &self.opts);
        self.report.fit_residual =
            prediction_residual(&self.model, obs.drives, obs.responses, &self.band);
        Ok(())
    }
    fn model_for(&mut self, next_level_db: f64) -> Result<ModelSource, String> {
        self.report.source = if Some(next_level_db) == self.level {
            ModelSource::LevelOwn
        } else {
            ModelSource::Predicted
        };
        Ok(self.report.source)
    }
    fn report(&self) -> ModelReport {
        self.report
    }
}

/// The sparse penalty on each pulse's change of the tracked parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sparse {
    /// Plain Gauss-Newton.
    None,
    /// A lasso penalty on each parameter's change (normalized), its weight
    /// `lambda_rel` times the smallest weight that would hold every parameter.
    Lasso {
        /// Relative weight.
        lambda_rel: f64,
    },
    /// A group-lasso penalty on each mode's (frequency, damping) change.
    GroupLasso {
        /// Relative weight.
        lambda_rel: f64,
    },
}

/// Options of [`PoleTrackModel`].
#[derive(Clone, Debug, PartialEq)]
pub struct TrackOptions {
    /// Gauss-Newton iterations a pulse.
    pub iterations: usize,
    /// Whether the first pulse of a new level gets a model extrapolated
    /// linearly in amplitude scale through the last two levels.
    pub extrapolate: bool,
    /// Whether the residues and residual terms are re-fitted from each pulse
    /// before the Gauss-Newton (by ridge least squares toward their current
    /// values, weight `refit_ridge` relative to the data's).
    pub refit_residues: bool,
    /// The residue re-fit's ridge.
    pub refit_ridge: f64,
    /// The sparse penalty.
    pub sparse: Sparse,
    /// The validity gate: the fallback's model is used from a pulse on which
    /// the tracked model's prediction residual is above `gate_ratio` times the
    /// fallback's, until a pulse on which it is below the fallback's divided
    /// by `gate_ratio`.
    /// `None` turns the gate off.
    pub gate_ratio: Option<f64>,
    /// No mode's frequency may move more than this fraction from its fitted
    /// value; an update that would is refused.
    pub max_frequency_change: f64,
    /// A same-level secant on top of the tracked model (`None`: off).
    pub secant: Option<SecantOptions>,
    /// When the modal model's relative residual against the identification
    /// on the band is above this, the run starts on the fallback (whose model
    /// is the identification) and the gate decides from the first pulse on.
    pub seed_residual_max: f64,
    /// The amplitude scale the modal model was identified at. When set, the
    /// identification is the first point of the level history, so the second
    /// level of a run is already extrapolated.
    pub identification_scale: Option<f64>,
}

impl Default for TrackOptions {
    fn default() -> Self {
        TrackOptions {
            iterations: 8,
            extrapolate: true,
            refit_residues: false,
            refit_ridge: 1e-3,
            sparse: Sparse::None,
            gate_ratio: Some(1.2),
            max_frequency_change: 0.25,
            secant: None,
            seed_residual_max: 0.05,
            identification_scale: None,
        }
    }
}

/// Pole tracking (module documentation).
pub struct PoleTrackModel {
    modal: ModalModel,
    theta0: Vec<f64>,
    base: MatrixStack,
    band: Vec<bool>,
    fs_hz: f64,
    opts: TrackOptions,
    hist: Vec<(f64, Vec<f64>)>,
    tracked: MatrixStack,
    current: MatrixStack,
    fallback: Box<dyn ModelProvider>,
    on_fallback: bool,
    level: Option<f64>,
    pairs: Vec<Pair>,
    report: ModelReport,
    gate_count: usize,
    rejected: usize,
    fitted: Option<MatrixStack>,
}

impl PoleTrackModel {
    /// Pole tracking of `modal` (fitted on `band`), with `base` the
    /// identification used outside the band and `fallback` the provider the
    /// gate hands over to.
    pub fn new(
        modal: ModalModel,
        base: MatrixStack,
        band: Vec<bool>,
        fs_hz: f64,
        opts: TrackOptions,
        fallback: Box<dyn ModelProvider>,
    ) -> Result<Self, String> {
        let (lines, n_c, n_d) = base.dims();
        if band.len() != lines || modal.n_c != n_c || modal.n_d != n_d {
            return Err("the modal model, the base model and the band do not agree".into());
        }
        if fallback.current().dims() != base.dims() {
            return Err("the fallback's model must have the base model's shape".into());
        }
        let tracked = modal.frf_in_band(&base, &band, fs_hz);
        let seed_residual = crate::modal::relative_residual(&modal, &base, &band, fs_hz);
        let on_fallback = seed_residual.is_nan() || seed_residual > opts.seed_residual_max;
        let current = if on_fallback {
            fallback.current().clone()
        } else {
            tracked.clone()
        };
        let mut report = ModelReport::default();
        if on_fallback {
            report.source = ModelSource::Fallback;
            report.gate_fired = true;
        }
        let hist = match opts.identification_scale {
            Some(s) if !on_fallback => vec![(s, modal.theta())],
            _ => Vec::new(),
        };
        Ok(PoleTrackModel {
            theta0: modal.theta(),
            modal,
            current,
            tracked,
            base,
            band,
            fs_hz,
            opts,
            hist,
            fallback,
            on_fallback,
            level: None,
            pairs: Vec::new(),
            report,
            gate_count: 0,
            rejected: 0,
            fitted: None,
        })
    }

    /// The modal model as now tracked.
    pub fn modal(&self) -> &ModalModel {
        &self.modal
    }

    /// How many pulses the gate handed to the fallback, and how many updates
    /// were refused.
    pub fn counts(&self) -> (usize, usize) {
        (self.gate_count, self.rejected)
    }

    fn band_lines(&self) -> Vec<(usize, f64)> {
        let lines = self.base.lines();
        let df = self.fs_hz / (2 * (lines - 1)) as f64;
        (0..lines)
            .filter(|&f| self.band[f])
            .map(|f| (f, TAU * f as f64 * df))
            .collect()
    }

    /// `||Y - H(theta) D|| / ||Y||` over the band, with `model`'s residues.
    fn residual(model: &ModalModel, lines: &[(usize, f64)], obs: &Observation<'_>) -> f64 {
        let (n_c, n_d) = (model.n_c, model.n_d);
        let mut h = vec![C64::new(0.0, 0.0); n_c * n_d];
        let (mut num, mut den) = (0.0, 0.0);
        for &(f, w) in lines {
            model.eval(w, &mut h);
            for r in 0..n_c {
                let mut hd = C64::new(0.0, 0.0);
                for c in 0..n_d {
                    hd += h[r * n_d + c] * obs.drives[c][f];
                }
                num += (obs.responses[r][f] - hd).norm_sqr();
                den += obs.responses[r][f].norm_sqr();
            }
        }
        if den > 0.0 {
            (num / den).sqrt()
        } else {
            f64::NAN
        }
    }

    /// The residue re-fit from one pulse (`Derivations.md` D12), row by row.
    fn refit_residues(
        &mut self,
        lines: &[(usize, f64)],
        obs: &Observation<'_>,
    ) -> Result<(), String> {
        let (n_c, n_d) = (self.modal.n_c, self.modal.n_d);
        let nm = self.modal.modes.len();
        let nb = nm + 2;
        let ncx = nb * n_d;
        let p = 2 * ncx;
        let fref = self.modal.f_ref_hz;
        for r in 0..n_c {
            let mut ata = vec![0.0; p * p];
            let mut atb = vec![0.0; p];
            let mut x0 = vec![0.0; p];
            for j in 0..n_d {
                let el = r * n_d + j;
                let vals = [self.modal.b0[el], self.modal.b2[el]];
                for (b, v) in vals.iter().enumerate() {
                    x0[2 * (b * n_d + j)] = v.re;
                    x0[2 * (b * n_d + j) + 1] = v.im;
                }
                for m in 0..nm {
                    let v = self.modal.modes[m].residue[el];
                    x0[2 * ((m + 2) * n_d + j)] = v.re;
                    x0[2 * ((m + 2) * n_d + j) + 1] = v.im;
                }
            }
            let mut phi = vec![C64::new(0.0, 0.0); ncx];
            for &(f, w) in lines {
                let back = C64::from_polar(1.0, w * self.modal.delays_s[r]);
                let yv = obs.responses[r][f] * back;
                let x = w / (TAU * fref);
                let mut fns = vec![C64::new(1.0, 0.0), C64::new(x * x, 0.0)];
                for m in &self.modal.modes {
                    fns.push(modal_q(w, TAU * m.fn_hz, m.zeta));
                }
                for (b, fb) in fns.iter().enumerate() {
                    for j in 0..n_d {
                        phi[b * n_d + j] = *fb * obs.drives[j][f];
                    }
                }
                // Real rows for Re and Im of y = sum phi_k c_k with c = a + j b.
                let rows = [
                    (
                        yv.re,
                        phi.iter().flat_map(|v| [v.re, -v.im]).collect::<Vec<_>>(),
                    ),
                    (
                        yv.im,
                        phi.iter().flat_map(|v| [v.im, v.re]).collect::<Vec<_>>(),
                    ),
                ];
                for (bv, row) in rows.iter() {
                    for a in 0..p {
                        if row[a] == 0.0 {
                            continue;
                        }
                        atb[a] += row[a] * bv;
                        for b in 0..p {
                            ata[a * p + b] += row[a] * row[b];
                        }
                    }
                }
            }
            let tr = (0..p).map(|i| ata[i * p + i]).sum::<f64>() / p as f64;
            let alpha = self.opts.refit_ridge * tr;
            for i in 0..p {
                ata[i * p + i] += alpha;
                atb[i] += alpha * x0[i];
            }
            let x = spd_solve(&ata, p, &atb, 0.0)?;
            for j in 0..n_d {
                let el = r * n_d + j;
                self.modal.b0[el] = C64::new(x[2 * j], x[2 * j + 1]);
                self.modal.b2[el] = C64::new(x[2 * (n_d + j)], x[2 * (n_d + j) + 1]);
                for m in 0..nm {
                    let k = 2 * ((m + 2) * n_d + j);
                    self.modal.modes[m].residue[el] = C64::new(x[k], x[k + 1]);
                }
            }
        }
        Ok(())
    }

    /// Gauss-Newton on `theta` from one pulse (`Derivations.md` D11), with the
    /// sparse penalty measured from `theta_ref`.
    fn gauss_newton(
        &self,
        theta_start: &[f64],
        theta_ref: &[f64],
        lines: &[(usize, f64)],
        obs: &Observation<'_>,
    ) -> Vec<f64> {
        let (n_c, n_d) = (self.modal.n_c, self.modal.n_d);
        let nm = self.modal.modes.len();
        let np = 2 * nm;
        let mut th = theta_start.to_vec();
        let mut h = vec![C64::new(0.0, 0.0); n_c * n_d];
        let mut v = vec![C64::new(0.0, 0.0); nm * n_c];
        for _ in 0..self.opts.iterations {
            let model = self.modal.with_theta(&th);
            let mut nrm = vec![0.0; np * np];
            let mut g = vec![0.0; np];
            let mut cols = vec![C64::new(0.0, 0.0); np * n_c];
            for &(f, w) in lines {
                model.eval(w, &mut h);
                for (m, mode) in model.modes.iter().enumerate() {
                    for r in 0..n_c {
                        let mut acc = C64::new(0.0, 0.0);
                        for c in 0..n_d {
                            acc += mode.residue[r * n_d + c] * obs.drives[c][f];
                        }
                        v[m * n_c + r] = acc * C64::from_polar(1.0, -w * model.delays_s[r]);
                    }
                    let (_, dfn, dz) = modal_q_derivs(w, mode.fn_hz, mode.zeta);
                    for r in 0..n_c {
                        cols[(2 * m) * n_c + r] = dfn * v[m * n_c + r];
                        cols[(2 * m + 1) * n_c + r] = dz * v[m * n_c + r];
                    }
                }
                for r in 0..n_c {
                    let mut hd = C64::new(0.0, 0.0);
                    for c in 0..n_d {
                        hd += h[r * n_d + c] * obs.drives[c][f];
                    }
                    let res = obs.responses[r][f] - hd;
                    for a in 0..np {
                        let ca = cols[a * n_c + r];
                        g[a] += (ca.conj() * res).re;
                        for b in a..np {
                            nrm[a * np + b] += (ca.conj() * cols[b * n_c + r]).re;
                        }
                    }
                }
            }
            for a in 0..np {
                for b in 0..a {
                    nrm[a * np + b] = nrm[b * np + a];
                }
            }
            let sc: Vec<f64> = (0..np)
                .map(|i| nrm[i * np + i].sqrt().max(1e-300))
                .collect();
            for a in 0..np {
                g[a] /= sc[a];
                for b in 0..np {
                    nrm[a * np + b] /= sc[a] * sc[b];
                }
            }
            let step = match self.opts.sparse {
                Sparse::None => spd_solve(&nrm, np, &g, 1e-10).unwrap_or_else(|_| vec![0.0; np]),
                Sparse::Lasso { lambda_rel } | Sparse::GroupLasso { lambda_rel } => {
                    let group = matches!(self.opts.sparse, Sparse::GroupLasso { .. });
                    // x = normalized (theta - theta_ref); x0 its value now.
                    let x0: Vec<f64> = (0..np).map(|i| sc[i] * (th[i] - theta_ref[i])).collect();
                    let x = sparse_solve(&nrm, &g, &x0, lambda_rel, group);
                    (0..np).map(|i| x[i] - x0[i]).collect()
                }
            };
            for i in 0..np {
                th[i] += step[i] / sc[i];
            }
            for t in th.chunks_mut(2) {
                t[1] = t[1].clamp(1e-4, 0.5);
            }
        }
        th
    }

    fn predict(&self, s_next: f64) -> Vec<f64> {
        let mut levels: Vec<(f64, Vec<f64>)> = Vec::new();
        for (s, th) in &self.hist {
            if let Some(e) = levels.iter_mut().find(|e| (e.0 - s).abs() < 1e-9) {
                e.1 = th.clone();
            } else {
                levels.push((*s, th.clone()));
            }
        }
        let last = match levels.last() {
            Some(l) => l,
            None => return self.modal.theta(),
        };
        if !self.opts.extrapolate || levels.len() < 2 || (s_next - last.0).abs() < 1e-9 {
            return self.modal.theta();
        }
        let prev = &levels[levels.len() - 2];
        let t = (s_next - last.0) / (last.0 - prev.0);
        let mut th: Vec<f64> = last
            .1
            .iter()
            .zip(&prev.1)
            .map(|(a, b)| a + (a - b) * t)
            .collect();
        for t in th.chunks_mut(2) {
            t[1] = t[1].clamp(1e-4, 0.5);
        }
        th
    }
}

/// Minimizes `0.5 x^T N x - x^T (g + N x0) + lambda sum |x_k|` (or the group
/// norm over (frequency, damping) pairs) by (block) coordinate descent, with
/// `lambda = lambda_rel max |(g + N x0)_k|` (the smallest weight holding every
/// parameter at zero change). `N` is unit-diagonal (column-normalized).
fn sparse_solve(n: &[f64], g: &[f64], x0: &[f64], lambda_rel: f64, group: bool) -> Vec<f64> {
    let np = g.len();
    let c: Vec<f64> = (0..np)
        .map(|i| g[i] + (0..np).map(|j| n[i * np + j] * x0[j]).sum::<f64>())
        .collect();
    let lmax = if group {
        c.chunks(2)
            .map(|p| (p[0] * p[0] + p[1] * p[1]).sqrt())
            .fold(0.0f64, f64::max)
    } else {
        c.iter().fold(0.0f64, |a, v| a.max(v.abs()))
    };
    let lambda = lambda_rel * lmax;
    let mut x = x0.to_vec();
    let bs = if group { 2 } else { 1 };
    for _ in 0..300 {
        let mut moved = 0.0f64;
        for blk in 0..np / bs {
            let idx: Vec<usize> = (blk * bs..blk * bs + bs).collect();
            // Majorize the block by its largest diagonal-dominant bound.
            let lip = idx
                .iter()
                .map(|&i| idx.iter().map(|&j| n[i * np + j].abs()).sum::<f64>())
                .fold(0.0f64, f64::max)
                .max(1e-12);
            let grad: Vec<f64> = idx
                .iter()
                .map(|&i| (0..np).map(|j| n[i * np + j] * x[j]).sum::<f64>() - c[i])
                .collect();
            let u: Vec<f64> = idx
                .iter()
                .zip(&grad)
                .map(|(&i, gr)| x[i] - gr / lip)
                .collect();
            let nu = u.iter().map(|v| v * v).sum::<f64>().sqrt();
            let shrink = if nu > 0.0 {
                (1.0 - lambda / lip / nu).max(0.0)
            } else {
                0.0
            };
            for (k, &i) in idx.iter().enumerate() {
                let nv = u[k] * shrink;
                moved = moved.max((nv - x[i]).abs());
                x[i] = nv;
            }
        }
        if moved < 1e-10 {
            break;
        }
    }
    x
}

impl ModelProvider for PoleTrackModel {
    fn current(&self) -> &MatrixStack {
        &self.current
    }

    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String> {
        let lines = self.band_lines();
        let s = level_scale(obs.level_db);
        let track_res = prediction_residual(&self.tracked, obs.drives, obs.responses, &self.band);
        let fb_res = prediction_residual(
            self.fallback.current(),
            obs.drives,
            obs.responses,
            &self.band,
        );
        self.fallback.observe(obs)?;
        let theta_ref = self.modal.theta();
        let before_modal = self.modal.clone();
        if self.opts.refit_residues {
            self.refit_residues(&lines, obs)?;
        }
        let before = Self::residual(&self.modal, &lines, obs);
        let th = self.gauss_newton(&theta_ref, &theta_ref, &lines, obs);
        let trial = self.modal.with_theta(&th);
        let after = Self::residual(&trial, &lines, obs);
        let in_bounds = th
            .chunks(2)
            .zip(self.theta0.chunks(2))
            .all(|(t, t0)| (t[0] / t0[0] - 1.0).abs() <= self.opts.max_frequency_change);
        if after.is_finite() && after <= before && in_bounds {
            self.modal = trial;
            self.report.fit_residual = after;
        } else {
            self.modal = before_modal;
            self.rejected += 1;
            self.report.fit_residual = Self::residual(&self.modal, &lines, obs);
        }
        self.hist.push((s, self.modal.theta()));
        if self.level != Some(obs.level_db) {
            self.pairs.clear();
            self.level = Some(obs.level_db);
        }
        if self.opts.secant.is_some() {
            self.pairs
                .push((obs.drives.to_vec(), obs.responses.to_vec()));
        }
        self.report.prediction_residual = track_res;
        self.report.fallback_residual = fb_res;
        if let Some(gr) = self.opts.gate_ratio {
            if track_res.is_finite() && fb_res.is_finite() {
                if track_res > gr * fb_res {
                    self.on_fallback = true;
                } else if track_res * gr <= fb_res {
                    self.on_fallback = false;
                }
            }
        }
        self.report.gate_fired = self.on_fallback;
        if self.on_fallback {
            self.gate_count += 1;
        }
        self.fitted = Some(if self.on_fallback {
            self.fallback.current().clone()
        } else {
            let own = self.modal.frf_in_band(&self.base, &self.band, self.fs_hz);
            match &self.opts.secant {
                Some(so) => secant_correct(&own, &self.pairs, so),
                None => own,
            }
        });
        Ok(())
    }

    fn model_for(&mut self, next_level_db: f64) -> Result<ModelSource, String> {
        self.fallback.model_for(next_level_db)?;
        let s_next = level_scale(next_level_db);
        let own = self.level == Some(next_level_db) || self.level.is_none();
        let th = self.predict(s_next);
        let model = self.modal.with_theta(&th);
        self.tracked = model.frf_in_band(&self.base, &self.band, self.fs_hz);
        let source = if self.level.is_none() {
            ModelSource::Initial
        } else if own {
            ModelSource::LevelOwn
        } else {
            ModelSource::Predicted
        };
        if self.on_fallback {
            self.current = self.fallback.current().clone();
            self.report.source = ModelSource::Fallback;
        } else {
            self.current = match (&self.opts.secant, own) {
                (Some(so), true) if !self.pairs.is_empty() => {
                    secant_correct(&self.tracked, &self.pairs, so)
                }
                _ => self.tracked.clone(),
            };
            self.report.source = source;
        }
        Ok(self.report.source)
    }

    fn report(&self) -> ModelReport {
        self.report
    }

    fn level_model(&self) -> Option<&MatrixStack> {
        self.fitted.as_ref()
    }

    fn radius(&self, next_level_db: f64, drops: &[f64]) -> Option<Vec<f64>> {
        let th = self.predict(level_scale(next_level_db));
        let lines = self.band_lines();
        let (n_c, n_d) = (self.modal.n_c, self.modal.n_d);
        let model = self.modal.with_theta(&th);
        let mut out = vec![0.0f64; self.base.lines()];
        let mut h0 = vec![C64::new(0.0, 0.0); n_c * n_d];
        let mut h1 = h0.clone();
        let perturbed: Vec<ModalModel> = drops
            .iter()
            .map(|d| {
                let t: Vec<f64> = th
                    .chunks(2)
                    .flat_map(|p| [p[0] * (1.0 - d), p[1]])
                    .collect();
                self.modal.with_theta(&t)
            })
            .collect();
        for &(f, w) in &lines {
            model.eval(w, &mut h0);
            for p in &perturbed {
                p.eval(w, &mut h1);
                let diff: Vec<C64> = h1.iter().zip(&h0).map(|(a, b)| a - b).collect();
                out[f] = out[f].max(spectral_norm(&diff, n_c, n_d));
            }
        }
        Some(out)
    }
}
