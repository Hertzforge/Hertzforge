//! Models for a run's first pulse on a plant whose resonances move with level,
//! a pulse-coded identification and the exponential weight (`Derivations.md`
//! D-R3-2 to D-R3-6). No law uses this module unless built with it.
//!
//! - [`family_inverse`]: the drive that is least wrong on average over a
//!   family of plant models, `G = (sum w_k H_k^H H_k + mu^2 I)^-1 sum w_k H_k^H`,
//!   and [`equivalent_model`], the single model whose inverse it is, so any law
//!   that inverts a model can play it.
//! - [`stretched_family`]: the identification with every resonance lowered by
//!   each of a set of fractions (the warp's frequency stretch with no decay).
//! - [`added_damping_model`]: the identification with a decay added to every
//!   pole (the warp's decay with no stretch): a shallower, wider notch in the
//!   inverse's drive at every resonance.
//! - [`coded_frf`]: the plant from copies of one pulse played with each
//!   drive's polarity coded, solved per line.
//! - [`exp_weight`]: records multiplied by `exp(-b (t - t0))`; a linear
//!   plant's convolution survives it exactly (with every pole moved by `b`).

// Per-line matrix arithmetic indexes several arrays by the same row and
// column; index loops are the clearer form here.
#![allow(clippy::needless_range_loop)]

use crate::lsq::{hpd_solve, spectral_norm};
use crate::provider::{prediction_residual, ModelProvider, ModelReport, ModelSource, Observation};
use crate::warp::WarpModel;
use crate::{MatrixStack, C64};

/// The family inverse on the masked lines, zero elsewhere. `weights` are
/// normalized to sum to one; the regularization is `lambda_rel` times the
/// largest member's largest singular value on the line.
pub fn family_inverse(
    models: &[MatrixStack],
    weights: &[f64],
    mask: &[bool],
    lambda_rel: f64,
) -> Result<MatrixStack, String> {
    let first = models
        .first()
        .ok_or("the family needs at least one model")?;
    let (lines, n_c, n_d) = first.dims();
    if models.iter().any(|m| m.dims() != (lines, n_c, n_d)) {
        return Err("the family's models must have one shape".into());
    }
    if weights.len() != models.len() || weights.iter().any(|w| !(*w >= 0.0 && w.is_finite())) {
        return Err("one finite, non-negative weight per model".into());
    }
    let wsum: f64 = weights.iter().sum();
    if wsum <= 0.0 {
        return Err("the weights must not all be zero".into());
    }
    if mask.len() != lines {
        return Err("the mask must have one flag per line".into());
    }
    if !(lambda_rel >= 0.0 && lambda_rel.is_finite()) {
        return Err("the regularization must be finite and not negative".into());
    }
    let zero = C64::new(0.0, 0.0);
    let mut g = MatrixStack::zeros(lines, n_d, n_c);
    for f in 0..lines {
        if !mask[f] {
            continue;
        }
        let mut a = vec![zero; n_d * n_d];
        let mut rhs = vec![zero; n_d * n_c];
        let mut smax = 0.0f64;
        for (m, &w) in models.iter().zip(weights) {
            let w = w / wsum;
            if w == 0.0 {
                continue;
            }
            let h = m.line(f);
            smax = smax.max(spectral_norm(h, n_c, n_d));
            for i in 0..n_d {
                for j in 0..n_d {
                    let mut s = zero;
                    for r in 0..n_c {
                        s += h[r * n_d + i].conj() * h[r * n_d + j];
                    }
                    a[i * n_d + j] += s * w;
                }
                for r in 0..n_c {
                    rhs[i * n_c + r] += h[r * n_d + i].conj() * w;
                }
            }
        }
        if smax == 0.0 {
            continue;
        }
        let mu = (lambda_rel * smax).max(1e-12 * smax);
        for i in 0..n_d {
            a[i * n_d + i] += C64::new(mu * mu, 0.0);
        }
        let x = hpd_solve(&a, n_d, &rhs, n_c)?;
        g.line_mut(f).copy_from_slice(&x);
    }
    Ok(g)
}

/// The model whose pseudo-inverse is `g` on the masked lines,
/// `M = G^H (G G^H)^-1` (`n_c x n_d`), and `base` elsewhere and wherever `g`
/// is zero. A law that inverts `M` with a small regularization plays `g`.
pub fn equivalent_model(
    g: &MatrixStack,
    base: &MatrixStack,
    mask: &[bool],
) -> Result<MatrixStack, String> {
    let (lines, n_d, n_c) = g.dims();
    if base.dims() != (lines, n_c, n_d) || mask.len() != lines {
        return Err("the inverse, the base model and the mask do not agree".into());
    }
    let zero = C64::new(0.0, 0.0);
    let mut m = base.clone();
    let mut eye = vec![zero; n_d * n_d];
    for i in 0..n_d {
        eye[i * n_d + i] = C64::new(1.0, 0.0);
    }
    for f in 0..lines {
        let gl = g.line(f);
        if !mask[f] || gl.iter().all(|v| v.norm() == 0.0) {
            continue;
        }
        // A = G G^H, n_d x n_d.
        let mut a = vec![zero; n_d * n_d];
        for i in 0..n_d {
            for j in 0..n_d {
                let mut s = zero;
                for r in 0..n_c {
                    s += gl[i * n_c + r] * gl[j * n_c + r].conj();
                }
                a[i * n_d + j] = s;
            }
        }
        let ainv = hpd_solve(&a, n_d, &eye, n_d)?;
        let out = m.line_mut(f);
        for r in 0..n_c {
            for j in 0..n_d {
                let mut s = zero;
                for i in 0..n_d {
                    s += gl[i * n_c + r].conj() * ainv[i * n_d + j];
                }
                out[r * n_d + j] = s;
            }
        }
    }
    Ok(m)
}

/// The warp's model with every resonance lowered by `drop` (a fraction): a
/// uniform stretch `1/a - 1 = -drop` at every knot, no added decay.
pub fn stretched(warp: &WarpModel, drop: f64) -> MatrixStack {
    let nk = warp.parameters().len() / 2;
    let mut x = vec![0.0; 2 * nk];
    for v in x[..nk].iter_mut() {
        *v = -drop;
    }
    warp.model_at(&x)
}

/// [`stretched`] for each of `drops`.
pub fn stretched_family(warp: &WarpModel, drops: &[f64]) -> Vec<MatrixStack> {
    drops.iter().map(|&d| stretched(warp, d)).collect()
}

/// The warp's model with a decay of `sigma` rad/s added to every pole (no
/// stretch): each resonance's damping ratio rises by about `sigma / omega`.
pub fn added_damping_model(warp: &WarpModel, sigma: f64) -> MatrixStack {
    let nk = warp.parameters().len() / 2;
    let mut x = vec![0.0; 2 * nk];
    for v in x[nk..].iter_mut() {
        *v = sigma;
    }
    warp.model_at(&x)
}

/// The plant on every line from `shots` of one pulse with coded polarities:
/// shot `k` played drive `j` as `codes[k][j] d_j`, `drive[j]` the spectrum of
/// `d_j` and `shots[k][i]` the spectrum of control `i`'s response. Per line the
/// codes are solved by least squares (they need full column rank); a drive
/// whose spectrum is under `floor_rel` of its own largest line leaves that
/// line's column at `prior`'s.
pub fn coded_frf(
    drive: &[Vec<C64>],
    shots: &[Vec<Vec<C64>>],
    codes: &[Vec<f64>],
    prior: &MatrixStack,
    floor_rel: f64,
) -> Result<MatrixStack, String> {
    let (lines, n_c, n_d) = prior.dims();
    let k = shots.len();
    if drive.len() != n_d || drive.iter().any(|d| d.len() != lines) {
        return Err("one drive spectrum per drive, on the prior's lines".into());
    }
    if codes.len() != k || codes.iter().any(|c| c.len() != n_d) || k < n_d {
        return Err(
            "one code row of every drive per shot, at least as many shots as drives".into(),
        );
    }
    if shots
        .iter()
        .any(|s| s.len() != n_c || s.iter().any(|r| r.len() != lines))
    {
        return Err("each shot needs one response spectrum per control".into());
    }
    // (C^T C)^-1 C^T, n_d x k.
    let mut ctc = vec![0.0; n_d * n_d];
    for row in codes {
        for i in 0..n_d {
            for j in 0..n_d {
                ctc[i * n_d + j] += row[i] * row[j];
            }
        }
    }
    let zero = C64::new(0.0, 0.0);
    let ctc_c: Vec<C64> = ctc.iter().map(|v| C64::new(*v, 0.0)).collect();
    let ct: Vec<C64> = (0..n_d)
        .flat_map(|i| (0..k).map(move |s| (i, s)))
        .map(|(i, s)| C64::new(codes[s][i], 0.0))
        .collect();
    if !full_rank(&ctc, n_d) {
        return Err("the codes are not of full column rank".into());
    }
    let pinv = hpd_solve(&ctc_c, n_d, &ct, k)?;
    let peak: Vec<f64> = drive
        .iter()
        .map(|d| d.iter().fold(0.0f64, |a, v| a.max(v.norm())))
        .collect();
    let mut h = prior.clone();
    for f in 0..lines {
        for i in 0..n_c {
            for j in 0..n_d {
                let dj = drive[j][f];
                if dj.norm() <= floor_rel * peak[j] || peak[j] == 0.0 {
                    continue;
                }
                let mut z = zero;
                for s in 0..k {
                    z += pinv[j * k + s] * shots[s][i][f];
                }
                h.set(f, i, j, z / dj);
            }
        }
    }
    Ok(h)
}

/// A model provider over a family of softened copies of the identification
/// (`Derivations.md` D-R3-8): the law plays the family inverse's equivalent
/// model, and the pulses re-weight the members by how well each predicted
/// them. The update is one-sided across levels: on a step up, members
/// stiffer than the best predictor of the last level are dropped and the
/// softer ones return to their prior weights (a plant pushed harder is
/// assumed no stiffer); at one level, the weights follow the evidence when
/// `within_level` is set.
pub struct FamilyModel {
    members: Vec<MatrixStack>,
    drops: Vec<f64>,
    prior: Vec<f64>,
    weights: Vec<f64>,
    posterior: Vec<f64>,
    band: Vec<bool>,
    base: MatrixStack,
    lambda_rel: f64,
    temperature: f64,
    within_level: bool,
    observed_level: Option<f64>,
    model: MatrixStack,
    report: ModelReport,
}

impl FamilyModel {
    /// The provider. `members[k]` is the identification lowered by
    /// `drops[k]`; `prior` its weight before any pulse; `temperature` how
    /// sharply a worse prediction loses weight (a member whose relative
    /// prediction residual is `(1 + t)` times the best's loses a factor `e`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base: MatrixStack,
        band: Vec<bool>,
        members: Vec<MatrixStack>,
        drops: Vec<f64>,
        prior: Vec<f64>,
        lambda_rel: f64,
        temperature: f64,
        within_level: bool,
    ) -> Result<Self, String> {
        if members.is_empty() || members.len() != drops.len() || members.len() != prior.len() {
            return Err("one drop and one prior weight per member".into());
        }
        if !(temperature > 0.0 && temperature.is_finite()) {
            return Err("the temperature must be positive".into());
        }
        let g = family_inverse(&members, &prior, &band, lambda_rel)?;
        let model = equivalent_model(&g, &base, &band)?;
        Ok(FamilyModel {
            weights: prior.clone(),
            posterior: prior.clone(),
            members,
            drops,
            prior,
            band,
            base,
            lambda_rel,
            temperature,
            within_level,
            observed_level: None,
            model,
            report: ModelReport::default(),
        })
    }

    /// The weights now in force, normalized.
    pub fn weights(&self) -> Vec<f64> {
        let s: f64 = self.weights.iter().sum();
        self.weights.iter().map(|w| w / s).collect()
    }

    fn rebuild(&mut self) -> Result<(), String> {
        let g = family_inverse(&self.members, &self.weights, &self.band, self.lambda_rel)?;
        self.model = equivalent_model(&g, &self.base, &self.band)?;
        Ok(())
    }
}

impl ModelProvider for FamilyModel {
    fn current(&self) -> &MatrixStack {
        &self.model
    }

    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String> {
        let e: Vec<f64> = self
            .members
            .iter()
            .map(|m| prediction_residual(m, obs.drives, obs.responses, &self.band))
            .collect();
        let e_min = e.iter().copied().fold(f64::INFINITY, f64::min);
        if !e_min.is_finite() {
            return Ok(());
        }
        // A residual floor of 1e-6 keeps an exact fit from dividing by zero.
        let e_ref = e_min.max(1e-6);
        let mut p: Vec<f64> = e
            .iter()
            .zip(&self.prior)
            .map(|(ek, w)| w * (-(ek / e_ref - e_min / e_ref) / self.temperature).exp())
            .collect();
        let s: f64 = p.iter().sum();
        for v in p.iter_mut() {
            *v /= s;
        }
        self.posterior = p;
        self.observed_level = Some(obs.level_db);
        self.report.prediction_residual =
            prediction_residual(&self.model, obs.drives, obs.responses, &self.band);
        self.report.fit_residual = e_min;
        Ok(())
    }

    fn model_for(&mut self, next_level_db: f64) -> Result<ModelSource, String> {
        let Some(level) = self.observed_level else {
            return Ok(ModelSource::Initial);
        };
        if next_level_db > level + 1e-9 {
            let best = (0..self.posterior.len())
                .max_by(|&a, &b| self.posterior[a].total_cmp(&self.posterior[b]))
                .unwrap_or(0);
            let floor = self.drops[best];
            self.weights = self
                .drops
                .iter()
                .zip(&self.prior)
                .map(|(d, w)| if *d + 1e-12 >= floor { *w } else { 0.0 })
                .collect();
            self.rebuild()?;
            self.report.source = ModelSource::Predicted;
        } else if self.within_level {
            self.weights = self.posterior.clone();
            self.rebuild()?;
            self.report.source = ModelSource::LevelOwn;
        }
        Ok(self.report.source)
    }

    fn report(&self) -> ModelReport {
        self.report
    }
}

/// The ladder's starting level from two probe pulses (`Derivations.md`
/// D-R3-7): `(level_db, mismatch)` at two levels, the mismatch
/// `||y - H d|| / ||y||` of each probe's measured response against the
/// model's prediction. The mismatch is extrapolated as a power of the
/// amplitude through the two probes, and the start is the highest level, in
/// whole dB, where it is predicted at most `threshold`, but never above
/// `ceiling_db` nor below the higher probe. No growth between the probes
/// gives the ceiling.
pub fn probe_start_level(probes: [(f64, f64); 2], threshold: f64, ceiling_db: f64) -> f64 {
    let [(l1, e1), (l2, e2)] = if probes[0].0 <= probes[1].0 {
        probes
    } else {
        [probes[1], probes[0]]
    };
    if !(e1 > 0.0 && e2 > 0.0 && l2 > l1) {
        return l1.min(ceiling_db);
    }
    if e2 >= threshold {
        return l1.min(ceiling_db);
    }
    if e2 <= e1 {
        return ceiling_db;
    }
    // e(L) = e2 (s / s2)^p, s = 10^(L / 20).
    let p = (e2 / e1).ln() / ((l2 - l1) / 20.0 * std::f64::consts::LN_10);
    let l = l2 + 20.0 * (threshold / e2).log10() / p;
    l.floor().clamp(l2, ceiling_db)
}

/// Whether the real `n x n` matrix has full rank (Gaussian elimination with
/// partial pivoting, pivots relative to the largest entry).
fn full_rank(a: &[f64], n: usize) -> bool {
    let mut m = a.to_vec();
    let scale = m.iter().fold(0.0f64, |x, v| x.max(v.abs()));
    if scale == 0.0 {
        return false;
    }
    for c in 0..n {
        let p = (c..n)
            .max_by(|&i, &j| m[i * n + c].abs().total_cmp(&m[j * n + c].abs()))
            .unwrap_or(c);
        if m[p * n + c].abs() <= 1e-10 * scale {
            return false;
        }
        for j in 0..n {
            m.swap(c * n + j, p * n + j);
        }
        for i in c + 1..n {
            let f = m[i * n + c] / m[c * n + c];
            for j in c..n {
                m[i * n + j] -= f * m[c * n + j];
            }
        }
    }
    true
}

/// Each record multiplied by `exp(-rate (t - t0_s))`, `t = index / fs_hz`.
pub fn exp_weight(rows: &[Vec<f64>], fs_hz: f64, rate: f64, t0_s: f64) -> Vec<Vec<f64>> {
    rows.iter()
        .map(|r| {
            r.iter()
                .enumerate()
                .map(|(i, v)| v * (-rate * (i as f64 / fs_hz - t0_s)).exp())
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::regularized_inverse;

    fn scalar_stack(values: &[C64]) -> MatrixStack {
        let mut m = MatrixStack::zeros(values.len(), 1, 1);
        for (f, v) in values.iter().enumerate() {
            m.set(f, 0, 0, *v);
        }
        m
    }

    #[test]
    fn mimo_shock_r3_family_of_one_is_the_regularized_inverse() {
        let mut h = MatrixStack::zeros(3, 2, 2);
        for f in 0..3 {
            h.set(f, 0, 0, C64::new(2.0 + f as f64, 0.5));
            h.set(f, 0, 1, C64::new(0.3, -0.1));
            h.set(f, 1, 0, C64::new(-0.2, 0.4));
            h.set(f, 1, 1, C64::new(1.0, 1.0));
        }
        let mask = [true, true, false];
        let a = family_inverse(std::slice::from_ref(&h), &[1.0], &mask, 1e-3).unwrap();
        let b = regularized_inverse(&h, &mask, 1e-3).unwrap();
        for f in 0..3 {
            for (x, y) in a.line(f).iter().zip(b.line(f)) {
                assert!((x - y).norm() < 1e-9, "{f}: {x} {y}");
            }
        }
        // And the equivalent model of an exact inverse is the model.
        let g = family_inverse(std::slice::from_ref(&h), &[1.0], &mask, 0.0).unwrap();
        let m = equivalent_model(&g, &h, &mask).unwrap();
        for f in 0..3 {
            for (x, y) in m.line(f).iter().zip(h.line(f)) {
                assert!((x - y).norm() < 1e-9);
            }
        }
    }

    /// One line, two members: a resonant one (gain 25) and an off-peak one
    /// (gain 6, other phase). The plain inverse of the off-peak member drives
    /// the resonant member 12 dB over; the family inverse holds every member
    /// under `1 / sqrt(w_k)` of the target (D-R3-4), here within 0.1 dB of it.
    #[test]
    fn mimo_shock_r3_family_inverse_bounds_every_member() {
        let peak = scalar_stack(&[C64::new(0.0, -25.0)]);
        let off = scalar_stack(&[C64::new(6.0, -1.0)]);
        let mask = [true];
        let plain = regularized_inverse(&off, &mask, 0.0).unwrap();
        let over = (peak.get(0, 0, 0) * plain.get(0, 0, 0)).norm();
        assert!(over > 3.9, "the plain inverse over-drives: {over}");
        let g = family_inverse(&[off.clone(), peak.clone()], &[1.0, 1.0], &mask, 0.0).unwrap();
        for m in [&peak, &off] {
            let r = (m.get(0, 0, 0) * g.get(0, 0, 0)).norm();
            assert!(r <= 2f64.sqrt() && r < 1.012, "member over-driven: {r}");
        }
    }

    #[test]
    fn mimo_shock_r3_coded_frf_recovers_a_linear_plant() {
        let (lines, n_c, n_d) = (5, 3, 2);
        let mut h = MatrixStack::zeros(lines, n_c, n_d);
        for f in 0..lines {
            for i in 0..n_c {
                for j in 0..n_d {
                    h.set(
                        f,
                        i,
                        j,
                        C64::new((1 + i + 2 * j + f) as f64, (i as f64) - (j as f64)),
                    );
                }
            }
        }
        let drive: Vec<Vec<C64>> = (0..n_d)
            .map(|j| {
                (0..lines)
                    .map(|f| C64::new(1.0 + (f + j) as f64, 0.5))
                    .collect()
            })
            .collect();
        let codes = vec![vec![1.0, 1.0], vec![1.0, -1.0]];
        let shots: Vec<Vec<Vec<C64>>> = codes
            .iter()
            .map(|c| {
                (0..n_c)
                    .map(|i| {
                        (0..lines)
                            .map(|f| (0..n_d).map(|j| h.get(f, i, j) * c[j] * drive[j][f]).sum())
                            .collect()
                    })
                    .collect()
            })
            .collect();
        let prior = MatrixStack::zeros(lines, n_c, n_d);
        let est = coded_frf(&drive, &shots, &codes, &prior, 1e-6).unwrap();
        for f in 0..lines {
            for (x, y) in est.line(f).iter().zip(h.line(f)) {
                assert!((x - y).norm() < 1e-9);
            }
        }
        // Equal codes cannot be solved.
        assert!(coded_frf(
            &drive,
            &shots,
            &[vec![1.0, 1.0], vec![1.0, 1.0]],
            &prior,
            0.0
        )
        .is_err());
    }

    /// Two members, one of which is the plant: the evidence moves the weight
    /// to it within a level, and a step up drops the stiffer member when the
    /// softer one predicted better (and keeps both when the stiffer one did).
    #[test]
    fn mimo_shock_r3_family_model_follows_the_evidence_one_sided() {
        let stiff = scalar_stack(&[C64::new(2.0, 0.0), C64::new(1.0, 0.0)]);
        let soft = scalar_stack(&[C64::new(1.0, 1.0), C64::new(0.5, 0.5)]);
        let band = vec![true, true];
        let mut fam = FamilyModel::new(
            stiff.clone(),
            band.clone(),
            vec![stiff.clone(), soft.clone()],
            vec![0.0, 0.1],
            vec![1.0, 1.0],
            0.0,
            0.1,
            true,
        )
        .unwrap();
        let d = vec![vec![C64::new(1.0, 0.0), C64::new(1.0, 0.0)]];
        let y_soft = vec![vec![soft.get(0, 0, 0), soft.get(1, 0, 0)]];
        let obs = Observation {
            level_db: -12.0,
            drives: &d,
            responses: &y_soft,
        };
        fam.observe(&obs).unwrap();
        fam.model_for(-12.0).unwrap();
        let w = fam.weights();
        assert!(w[1] > 0.999, "the plant's member takes the weight: {w:?}");
        fam.model_for(-9.0).unwrap();
        assert_eq!(fam.weights(), vec![0.0, 1.0]);
        // The stiff member predicted: on a step up both return to the prior.
        let y_stiff = vec![vec![stiff.get(0, 0, 0), stiff.get(1, 0, 0)]];
        fam.observe(&Observation {
            level_db: -9.0,
            drives: &d,
            responses: &y_stiff,
        })
        .unwrap();
        fam.model_for(-6.0).unwrap();
        assert_eq!(fam.weights(), vec![0.5, 0.5]);
    }

    #[test]
    fn mimo_shock_r3_probe_start_level_extrapolates_the_mismatch() {
        // Mismatch growing as the amplitude (p = 1): 0.05 at -40, 0.0794 at -36
        // reaches 0.15 at -30.46 dB: start -31.
        let e2 = 0.05 * 10f64.powf(4.0 / 20.0);
        assert_eq!(
            probe_start_level([(-40.0, 0.05), (-36.0, e2)], 0.15, -12.0),
            -31.0
        );
        // Order does not matter.
        assert_eq!(
            probe_start_level([(-36.0, e2), (-40.0, 0.05)], 0.15, -12.0),
            -31.0
        );
        // No growth: the ceiling.
        assert_eq!(
            probe_start_level([(-40.0, 0.11), (-36.0, 0.10)], 0.15, -12.0),
            -12.0
        );
        // Already over at the higher probe: the lower probe's level.
        assert_eq!(
            probe_start_level([(-40.0, 0.1), (-36.0, 0.2)], 0.15, -12.0),
            -40.0
        );
        // Never above the ceiling.
        assert_eq!(
            probe_start_level([(-40.0, 0.01), (-36.0, 0.011)], 0.15, -12.0),
            -12.0
        );
    }

    /// `(h e^-bt) * (d e^-bt) = (h * d) e^-bt` for a linear convolution.
    #[test]
    fn mimo_shock_r3_exponential_weight_keeps_a_convolution() {
        let h: Vec<f64> = (0..8)
            .map(|i| (0.7f64).powi(i) * (i as f64).cos())
            .collect();
        let d: Vec<f64> = (0..20).map(|i| ((i * 7 % 5) as f64) - 2.0).collect();
        let conv = |a: &[f64], b: &[f64]| -> Vec<f64> {
            let mut y = vec![0.0; a.len() + b.len() - 1];
            for (i, x) in a.iter().enumerate() {
                for (j, z) in b.iter().enumerate() {
                    y[i + j] += x * z;
                }
            }
            y
        };
        let fs = 10.0;
        let b = 1.3;
        let y = conv(&h, &d);
        let yw = exp_weight(std::slice::from_ref(&y), fs, b, 0.0).remove(0);
        let hw = exp_weight(std::slice::from_ref(&h), fs, b, 0.0).remove(0);
        let dw = exp_weight(std::slice::from_ref(&d), fs, b, 0.0).remove(0);
        for (a, c) in conv(&hw, &dw).iter().zip(&yw) {
            assert!((a - c).abs() < 1e-12);
        }
    }
}
