//! The family inverse's softening set from the test's own pulses
//! (`Derivations.md` D-R4-1 to D-R4-4). No law uses this module unless built
//! with it.
//!
//! - [`fit_softening`]: per band of frequency, the one downward stretch of the
//!   identification that best predicts a level's pulses, with the response
//!   amplitude the band reached.
//! - [`extrapolate_drop`]: a band's drop carried to a higher amplitude as a
//!   power of the amplitude, the power bounded.
//! - [`SofteningFamily`]: a model provider that plays the identification for
//!   the first (probe) levels, then the family inverse over the identification
//!   lowered by fractions of an assumed drop per band, set from the fits with
//!   a margin and a ceiling, either once (from the probes) or at every level.

// Per-line matrix arithmetic indexes several arrays by the same row and
// column; index loops are the clearer form here.
#![allow(clippy::needless_range_loop)]

use crate::first_pulse::{equivalent_model, family_inverse};
use crate::provider::{prediction_residual, ModelProvider, ModelReport, ModelSource, Observation};
use crate::warp::WarpModel;
use crate::{level_scale, MatrixStack, C64};

/// Which band each knot and each line belongs to.
#[derive(Clone, Debug, PartialEq)]
pub struct Bands {
    knot_band: Vec<usize>,
    line_band: Vec<Option<usize>>,
    n: usize,
}

impl Bands {
    /// Bands of `octaves` octaves from the warp's lowest knot. Lines outside
    /// `mask` or outside the knots' span belong to no band.
    pub fn octaves(warp: &WarpModel, mask: &[bool], octaves: f64) -> Result<Bands, String> {
        if !(octaves > 0.0 && octaves.is_finite()) {
            return Err("a band must span a positive number of octaves".into());
        }
        let knots = warp.knot_hz();
        let (lo, hi) = (knots[0], knots[knots.len() - 1]);
        let n = (((hi / lo).log2() / octaves).floor() as usize + 1).max(1);
        let band_of = |f: f64| (((f / lo).log2() / octaves).floor().max(0.0) as usize).min(n - 1);
        let df = warp.fs_hz() / (2 * (mask.len() - 1)) as f64;
        Ok(Bands {
            knot_band: knots.iter().map(|&f| band_of(f)).collect(),
            line_band: mask
                .iter()
                .enumerate()
                .map(|(i, &on)| {
                    let f = i as f64 * df;
                    (on && f > lo && f < hi).then(|| band_of(f))
                })
                .collect(),
            n,
        })
    }

    /// One band over the whole span (the one-number estimate).
    pub fn single(warp: &WarpModel, mask: &[bool]) -> Result<Bands, String> {
        let mut b = Bands::octaves(warp, mask, 1.0)?;
        b.knot_band.iter_mut().for_each(|v| *v = 0);
        b.line_band.iter_mut().flatten().for_each(|v| *v = 0);
        b.n = 1;
        Ok(b)
    }

    /// How many bands.
    pub fn len(&self) -> usize {
        self.n
    }

    /// Whether there are no bands.
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The warp's parameter vector lowering band `b`'s resonances by
    /// `frac * drops[b]` (stretch `1/a - 1 = -frac drops[b]` at its knots, no
    /// decay).
    pub fn stretch(&self, drops: &[f64], frac: f64) -> Vec<f64> {
        let nk = self.knot_band.len();
        let mut x = vec![0.0; 2 * nk];
        for (k, &b) in self.knot_band.iter().enumerate() {
            x[k] = -frac * drops[b];
        }
        x
    }
}

/// One pulse, as the law sees it: drive and response spectra divided by the
/// level's scale, and that scale.
#[derive(Clone, Debug)]
pub struct PulseSpectra {
    /// One spectrum per drive, divided by the level's scale.
    pub drives: Vec<Vec<C64>>,
    /// One spectrum per control, divided by the level's scale.
    pub responses: Vec<Vec<C64>>,
    /// The level's amplitude scale.
    pub scale: f64,
}

/// The fit's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct FitOptions {
    /// The coarse grid of drops, fractions: from, to, step.
    pub coarse: (f64, f64, f64),
    /// The fine search around each band's coarse best: half width and step.
    pub fine: (f64, f64),
    /// A band's drop counts only when it lowers the band's prediction
    /// residual to at most `1 - min_gain` of the identification's.
    pub min_gain: f64,
    /// A band with under this fraction of the largest band's response energy
    /// is not fitted (its drop is zero).
    pub min_energy_rel: f64,
}

impl Default for FitOptions {
    fn default() -> Self {
        FitOptions {
            coarse: (-0.05, 0.40, 0.01),
            fine: (0.01, 0.001),
            min_gain: 0.1,
            min_energy_rel: 1e-4,
        }
    }
}

/// What [`fit_softening`] found, per band.
#[derive(Clone, Debug, PartialEq)]
pub struct SofteningFit {
    /// The drop, a fraction (zero where not significant).
    pub drops: Vec<f64>,
    /// The drop at the residual's minimum before the significance test.
    pub raw_drops: Vec<f64>,
    /// The response's amplitude in the band over the full-level reference's
    /// (the level's scale times the measured-to-target ratio), averaged over
    /// the pulses.
    pub amplitude: Vec<f64>,
    /// Relative prediction residual at the identification.
    pub residual_at_zero: Vec<f64>,
    /// Relative prediction residual at the drop's minimum.
    pub residual: Vec<f64>,
}

/// Per band `(sum |Y - H D|^2, sum |Y|^2)` over the pulses.
fn band_sums(model: &MatrixStack, bands: &Bands, pulses: &[PulseSpectra]) -> Vec<(f64, f64)> {
    let (_, n_c, n_d) = model.dims();
    let mut out = vec![(0.0, 0.0); bands.n];
    for p in pulses {
        for (f, b) in bands.line_band.iter().enumerate() {
            let Some(b) = *b else { continue };
            let line = model.line(f);
            for r in 0..n_c {
                let mut hd = C64::new(0.0, 0.0);
                for c in 0..n_d {
                    hd += line[r * n_d + c] * p.drives[c][f];
                }
                out[b].0 += (p.responses[r][f] - hd).norm_sqr();
                out[b].1 += p.responses[r][f].norm_sqr();
            }
        }
    }
    out
}

fn rel_residual(s: (f64, f64)) -> f64 {
    if s.1 > 0.0 {
        (s.0 / s.1).sqrt()
    } else {
        f64::NAN
    }
}

/// The per-band drop that best predicts `pulses` (`Derivations.md` D-R4-1):
/// a common drop over a coarse grid, each band taking its own best, then a
/// fine search around each band's best (the bands searched together, each
/// keeping its own minimum). `reference` is the full-level reference's
/// spectra, one row per control.
pub fn fit_softening(
    warp: &WarpModel,
    bands: &Bands,
    pulses: &[PulseSpectra],
    reference: &[Vec<C64>],
    opts: &FitOptions,
) -> Result<SofteningFit, String> {
    if pulses.is_empty() {
        return Err("a fit needs at least one pulse".into());
    }
    let (from, to, step) = opts.coarse;
    if !(step > 0.0 && to > from && opts.fine.1 > 0.0 && opts.fine.0 >= 0.0) {
        return Err("the fit's grids must have positive steps".into());
    }
    let nb = bands.n;
    let base = band_sums(
        &warp.model_at(&bands.stretch(&vec![0.0; nb], 0.0)),
        bands,
        pulses,
    );
    let r0: Vec<f64> = base.iter().map(|s| rel_residual(*s)).collect();
    let mut best: Vec<(f64, f64)> = r0.iter().map(|r| (*r, 0.0)).collect();
    let consider = |drops: &[f64], best: &mut Vec<(f64, f64)>| {
        let sums = band_sums(&warp.model_at(&bands.stretch(drops, 1.0)), bands, pulses);
        for b in 0..nb {
            let r = rel_residual(sums[b]);
            if r < best[b].0 {
                best[b] = (r, drops[b]);
            }
        }
    };
    let n_coarse = ((to - from) / step).round() as usize;
    for i in 0..=n_coarse {
        let d = from + i as f64 * step;
        consider(&vec![d; nb], &mut best);
    }
    let centre: Vec<f64> = best.iter().map(|b| b.1).collect();
    let n_fine = (opts.fine.0 / opts.fine.1).round() as i64;
    for i in -n_fine..=n_fine {
        let off = i as f64 * opts.fine.1;
        let drops: Vec<f64> = centre.iter().map(|c| c + off).collect();
        consider(&drops, &mut best);
    }
    // Amplitude per band: the measured response over the reference's, times
    // the level's scale, averaged over the pulses.
    let mut ref_e = vec![0.0; nb];
    for (f, b) in bands.line_band.iter().enumerate() {
        if let Some(b) = *b {
            for row in reference {
                ref_e[b] += row[f].norm_sqr();
            }
        }
    }
    let mut amplitude = vec![0.0; nb];
    for p in pulses {
        let sums = band_sums(
            &MatrixStack::zeros(p.drives[0].len(), p.responses.len(), p.drives.len()),
            bands,
            std::slice::from_ref(p),
        );
        for b in 0..nb {
            if ref_e[b] > 0.0 {
                amplitude[b] += p.scale * (sums[b].1 / ref_e[b]).sqrt() / pulses.len() as f64;
            }
        }
    }
    let e_max = base.iter().fold(0.0f64, |a, s| a.max(s.1));
    let raw: Vec<f64> = best.iter().map(|b| b.1).collect();
    let drops: Vec<f64> = (0..nb)
        .map(|b| {
            let significant = best[b].0 <= (1.0 - opts.min_gain) * r0[b];
            if base[b].1 < opts.min_energy_rel * e_max || !significant {
                0.0
            } else {
                raw[b]
            }
        })
        .collect();
    Ok(SofteningFit {
        drops,
        raw_drops: raw,
        amplitude,
        residual_at_zero: r0,
        residual: best.iter().map(|b| b.0).collect(),
    })
}

/// A drop `d2` measured at amplitude `a2` carried to amplitude `at`
/// (`Derivations.md` D-R4-2). With an earlier point `(a1, d1)` the power is
/// `ln(d2 / d1) / ln(a2 / a1)`, bounded to `[0, p_max]`; with none, or when
/// `d1` is not positive, the power is `p_max`. A drop that is not positive
/// stays zero; the drop is never carried downward (`at <= a2` gives `d2`).
pub fn extrapolate_drop(earlier: Option<(f64, f64)>, a2: f64, d2: f64, at: f64, p_max: f64) -> f64 {
    if d2 <= 0.0 || a2.is_nan() || a2 <= 0.0 {
        return 0.0;
    }
    if at <= a2 {
        return d2;
    }
    let p = match earlier {
        Some((a1, d1)) if d1 > 0.0 && a1 > 0.0 && (a2 / a1 - 1.0).abs() > 1e-6 => {
            ((d2 / d1).ln() / (a2 / a1).ln()).clamp(0.0, p_max)
        }
        _ => p_max,
    };
    d2 * (at / a2).powf(p)
}

/// How the assumed drop moves after the probes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Follow {
    /// Set on each step up from the probe levels' fits until the step that
    /// reaches the ladder's first level ([`SofteningPlan::ladder_start_db`]),
    /// then kept (only ever widened before that). With no ladder start, set
    /// once on the first step after the probes.
    Probes,
    /// Re-set on every step up (and, with `within_level`, after every pulse
    /// at a level) from the fits extrapolated to the next level, never
    /// narrowing by more than the factor `narrow` (in `[0, 1]`) at a time;
    /// `narrow = 1` only ever widens.
    Levels {
        /// The least fraction of the old drop a change keeps.
        narrow: f64,
        /// Whether the drop is also re-set after every pulse at a level.
        within_level: bool,
    },
    /// Re-set on every step up, only ever widened there; after every pulse
    /// at a level, narrowed (never widened) toward that level's own fit, to
    /// no less than `narrow` of the old drop, and reported as the level's own
    /// model so a law that resets its learning on such a model does.
    Narrowing {
        /// The least fraction of the old drop a change keeps.
        narrow: f64,
    },
}

/// The method's settings. One setting serves every plant.
#[derive(Clone, Debug, PartialEq)]
pub struct SofteningPlan {
    /// The first this many levels are probes, played on the identification.
    pub probe_levels: usize,
    /// The family's largest drop over the extrapolated fit (D-R4-1: at
    /// least `(2 + p) / 2` for a drop that follows the envelope as `r^p`).
    pub margin: f64,
    /// The largest power of the amplitude a drop is extrapolated with.
    pub p_max: f64,
    /// The largest drop the family assumes in any band.
    pub ceiling: f64,
    /// Members as fractions of the assumed drop.
    pub members: Vec<f64>,
    /// Once or followed.
    pub follow: Follow,
    /// The ladder's first level, dB: in [`Follow::Probes`], levels below it
    /// (the probes, and any level a ladder guard inserts) are probes.
    pub ladder_start_db: Option<f64>,
    /// Octaves per band; `None`: one band (one number) for the whole span.
    pub band_octaves: Option<f64>,
    /// The fit's settings.
    pub fit: FitOptions,
    /// The family inverse's regularization.
    pub lambda_rel: f64,
    /// When set, the margin is measured instead of `margin`
    /// (`Derivations.md` D-R4-6): `(2 + p) / 2` for the power `p` the drop
    /// is carried with, times the largest ratio so far of a level's fitted
    /// drop over what the previous levels predicted for it (at least one).
    pub data_margin: bool,
}

impl Default for SofteningPlan {
    fn default() -> Self {
        SofteningPlan {
            probe_levels: 2,
            margin: 1.5,
            p_max: 1.0,
            ceiling: 0.12,
            members: vec![0.0, 0.5, 1.0],
            follow: Follow::Probes,
            ladder_start_db: None,
            band_octaves: Some(1.0),
            fit: FitOptions::default(),
            lambda_rel: 1e-3,
            data_margin: false,
        }
    }
}

/// One level's record: the level, its fit (over every pulse there).
#[derive(Clone, Debug)]
pub struct LevelFit {
    /// The level, dB.
    pub level_db: f64,
    /// The fit over every pulse at the level.
    pub fit: SofteningFit,
}

/// The model provider of the module documentation.
pub struct SofteningFamily {
    warp: WarpModel,
    bands: Bands,
    base: MatrixStack,
    band: Vec<bool>,
    reference: Vec<Vec<C64>>,
    plan: SofteningPlan,
    pulses: Vec<PulseSpectra>,
    levels: Vec<LevelFit>,
    assumed: Vec<f64>,
    set_once: bool,
    frozen: bool,
    model: MatrixStack,
    report: ModelReport,
    log: Vec<(f64, Vec<f64>)>,
    /// Per band, what the last step predicted: `(a2, d2, p)`.
    pending: Option<Vec<(f64, f64, f64)>>,
    pred_error: f64,
    errors: Vec<(f64, f64)>,
}

impl SofteningFamily {
    /// The provider. `warp` is built on the identification `base`; `band` the
    /// lines the law drives; `reference` the full-level reference's spectra.
    pub fn new(
        warp: WarpModel,
        base: MatrixStack,
        band: Vec<bool>,
        reference: Vec<Vec<C64>>,
        plan: SofteningPlan,
    ) -> Result<Self, String> {
        if !(plan.margin > 0.0 && plan.p_max >= 0.0 && plan.ceiling >= 0.0) {
            return Err(
                "the margin must be positive and the power and ceiling not negative".into(),
            );
        }
        if plan.members.is_empty() {
            return Err("the family needs at least one member".into());
        }
        if let Follow::Levels { narrow, .. } | Follow::Narrowing { narrow } = plan.follow {
            if !(0.0..=1.0).contains(&narrow) {
                return Err("the narrowing factor must be in [0, 1]".into());
            }
        }
        let bands = match plan.band_octaves {
            Some(o) => Bands::octaves(&warp, &band, o)?,
            None => Bands::single(&warp, &band)?,
        };
        let nb = bands.len();
        Ok(SofteningFamily {
            warp,
            bands,
            model: base.clone(),
            base,
            band,
            reference,
            plan,
            pulses: Vec::new(),
            levels: Vec::new(),
            assumed: vec![0.0; nb],
            set_once: false,
            frozen: false,
            report: ModelReport::default(),
            log: Vec::new(),
            pending: None,
            pred_error: 1.0,
            errors: Vec::new(),
        })
    }

    /// Start from an assumed drop of `drop` in every band (a hand-set family)
    /// instead of the probes: the drop is in force from the first pulse and
    /// the following rules apply from there.
    pub fn start_at(&mut self, drop: f64) -> Result<(), String> {
        self.assumed = vec![drop.clamp(0.0, 1.0); self.bands.len()];
        self.set_once = true;
        self.frozen = true;
        self.plan.probe_levels = 0;
        self.log.push((f64::NAN, self.assumed.clone()));
        self.rebuild()
    }

    /// The assumed drop per band now in force.
    pub fn assumed(&self) -> &[f64] {
        &self.assumed
    }

    /// Each level's fit, in the order played.
    pub fn levels(&self) -> &[LevelFit] {
        &self.levels
    }

    /// Every change of the assumed drop: the level it was set for, the drops.
    pub fn changes(&self) -> &[(f64, Vec<f64>)] {
        &self.log
    }

    /// The largest ratio so far of a level's fitted drop over its prediction
    /// (at least one), and every measured ratio with its level.
    pub fn prediction_error(&self) -> (f64, &[(f64, f64)]) {
        (self.pred_error, &self.errors)
    }

    /// The band edges' knot bands (for reports).
    pub fn bands(&self) -> &Bands {
        &self.bands
    }

    /// The drop per band extrapolated to `level_db` from the last two levels'
    /// fits, times the margin, at most the ceiling; and per band the
    /// prediction's `(a2, d2, p)` before the margin.
    fn target_with(&self, level_db: f64) -> (Vec<f64>, Vec<(f64, f64, f64)>) {
        let nb = self.bands.len();
        let at = level_scale(level_db);
        let n = self.levels.len();
        let last = &self.levels[n - 1].fit;
        let prev = (n >= 2).then(|| &self.levels[n - 2].fit);
        let mut pred = Vec::with_capacity(nb);
        let t = (0..nb)
            .map(|b| {
                let (a2, d2) = (last.amplitude[b], last.drops[b]);
                let earlier = prev.map(|p| (p.amplitude[b], p.drops[b]));
                // With no earlier drop to measure a power from, the power is
                // the joint model's small-amplitude bound, 1, under `p_max`.
                // Under the data margin a power is measured only from drops the
                // fit resolves to a tenth (ten fine steps).
                let res = if self.plan.data_margin {
                    10.0 * self.plan.fit.fine.1
                } else {
                    0.0
                };
                let p = match earlier {
                    Some((a1, d1))
                        if d1 > res
                            && d2 > res
                            && d1 > 0.0
                            && d2 > 0.0
                            && a1 > 0.0
                            && (a2 / a1 - 1.0).abs() > 1e-6 =>
                    {
                        ((d2 / d1).ln() / (a2 / a1).ln()).clamp(0.0, self.plan.p_max)
                    }
                    _ => self.plan.p_max.min(1.0),
                };
                let d = extrapolate_drop(None, a2, d2, at, p);
                pred.push((a2, d2, p));
                let m = if self.plan.data_margin {
                    (2.0 + p) / 2.0 * self.pred_error
                } else {
                    self.plan.margin
                };
                (m * d).clamp(0.0, self.plan.ceiling)
            })
            .collect();
        (t, pred)
    }

    fn target(&self, level_db: f64) -> Vec<f64> {
        self.target_with(level_db).0
    }

    fn set(&mut self, level_db: f64, target: Vec<f64>, narrow: f64) -> Result<bool, String> {
        let new: Vec<f64> = target
            .iter()
            .zip(&self.assumed)
            .map(|(t, old)| t.max(narrow * old))
            .collect();
        let moved = new
            .iter()
            .zip(&self.assumed)
            .any(|(a, b)| (a - b).abs() > 1e-4);
        if !moved {
            return Ok(false);
        }
        self.assumed = new;
        self.log.push((level_db, self.assumed.clone()));
        self.rebuild()?;
        Ok(true)
    }

    fn rebuild(&mut self) -> Result<(), String> {
        if self.assumed.iter().all(|d| *d < 1e-4) {
            self.model = self.base.clone();
            return Ok(());
        }
        let members: Vec<MatrixStack> = self
            .plan
            .members
            .iter()
            .map(|&f| self.warp.model_at(&self.bands.stretch(&self.assumed, f)))
            .collect();
        let w = vec![1.0; members.len()];
        let g = family_inverse(&members, &w, &self.band, self.plan.lambda_rel)?;
        self.model = equivalent_model(&g, &self.base, &self.band)?;
        Ok(())
    }
}

impl ModelProvider for SofteningFamily {
    fn current(&self) -> &MatrixStack {
        &self.model
    }

    fn observe(&mut self, obs: &Observation<'_>) -> Result<(), String> {
        let new_level = self
            .levels
            .last()
            .map(|l| (l.level_db - obs.level_db).abs() > 1e-9)
            .unwrap_or(true);
        if new_level {
            self.pulses.clear();
        }
        self.pulses.push(PulseSpectra {
            drives: obs.drives.to_vec(),
            responses: obs.responses.to_vec(),
            scale: level_scale(obs.level_db),
        });
        let fit = fit_softening(
            &self.warp,
            &self.bands,
            &self.pulses,
            &self.reference,
            &self.plan.fit,
        )?;
        self.report.prediction_residual =
            prediction_residual(&self.model, obs.drives, obs.responses, &self.band);
        let (num, den) = fit
            .residual
            .iter()
            .zip(&fit.residual_at_zero)
            .fold((0.0, 0.0), |a, (r, _)| (a.0 + r, a.1 + 1.0));
        self.report.fit_residual = if den > 0.0 { num / den } else { f64::NAN };
        if new_level {
            // The last step's prediction against what this level measured,
            // at the amplitude it reached.
            if let Some(pred) = self.pending.take() {
                for (b, &(a2, d2, p)) in pred.iter().enumerate() {
                    let d = fit.drops[b];
                    let want = extrapolate_drop(None, a2, d2, fit.amplitude[b], p);
                    let res = 10.0 * self.plan.fit.fine.1;
                    if d > res && want > res {
                        let r = d / want;
                        self.errors.push((obs.level_db, r));
                        self.pred_error = self.pred_error.max(r);
                    }
                }
            }
            self.levels.push(LevelFit {
                level_db: obs.level_db,
                fit,
            });
        } else if let Some(l) = self.levels.last_mut() {
            l.fit = fit;
        }
        Ok(())
    }

    fn model_for(&mut self, next_level_db: f64) -> Result<ModelSource, String> {
        let Some(level) = self.levels.last().map(|l| l.level_db) else {
            return Ok(ModelSource::Initial);
        };
        let step_up = next_level_db > level + 1e-9;
        if self.levels.len() < self.plan.probe_levels || (!step_up && !self.set_once) {
            self.report.source = if self.set_once {
                ModelSource::Predicted
            } else {
                ModelSource::Initial
            };
            return Ok(self.report.source);
        }
        match self.plan.follow {
            Follow::Probes => {
                if step_up && !self.frozen {
                    let t = self.target(next_level_db);
                    let n = if self.set_once { 1.0 } else { 0.0 };
                    self.set(next_level_db, t, n)?;
                    self.set_once = true;
                    self.frozen = self
                        .plan
                        .ladder_start_db
                        .is_none_or(|l| next_level_db >= l - 1e-9);
                    self.report.source = ModelSource::Predicted;
                }
            }
            Follow::Levels {
                narrow,
                within_level,
            } => {
                if step_up {
                    let t = self.target(next_level_db);
                    // The first setting starts from nothing.
                    let n = if self.set_once { narrow } else { 0.0 };
                    self.set(next_level_db, t, n)?;
                    self.set_once = true;
                    self.report.source = ModelSource::Predicted;
                } else if within_level {
                    let t = self.target(next_level_db);
                    self.report.source = if self.set(next_level_db, t, narrow)? {
                        ModelSource::LevelOwn
                    } else {
                        ModelSource::Predicted
                    };
                }
            }
            Follow::Narrowing { narrow } => {
                if step_up {
                    let (t, pred) = self.target_with(next_level_db);
                    self.pending = Some(pred);
                    let n = if self.set_once { 1.0 } else { 0.0 };
                    self.set(next_level_db, t, n)?;
                    self.set_once = true;
                    self.report.source = ModelSource::Predicted;
                } else {
                    // Narrow only: never above the drop in force.
                    let t: Vec<f64> = self
                        .target(next_level_db)
                        .into_iter()
                        .zip(&self.assumed)
                        .map(|(t, old)| t.min(*old))
                        .collect();
                    self.report.source = if self.set(next_level_db, t, narrow)? {
                        ModelSource::LevelOwn
                    } else {
                        ModelSource::Predicted
                    };
                }
            }
        }
        Ok(self.report.source)
    }

    fn report(&self) -> ModelReport {
        self.report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warp::WarpOptions;
    use std::f64::consts::TAU;

    const FS: f64 = 12_800.0;
    const N: usize = 8192;

    /// A 2 x 2 acceleration plant with modes at `f[k] (1 - drop[k])`, each
    /// loading both drives differently.
    fn plant(fk: &[f64], drop: &[f64]) -> MatrixStack {
        let lines = N / 2 + 1;
        let df = FS / N as f64;
        let shapes = [[1.0, 0.6, -0.4, 0.8], [0.7, -0.5, 0.9, 0.3]];
        let mut h = MatrixStack::zeros(lines, 2, 2);
        for i in 0..lines {
            let w = TAU * i as f64 * df;
            for (k, (&f0, &d)) in fk.iter().zip(drop).enumerate() {
                let wn = TAU * f0 * (1.0 - d);
                let s = C64::new(0.0, w);
                let resp = s * s / (s * s + 2.0 * 0.02 * wn * s + wn * wn);
                let sh = shapes[k % 2];
                for r in 0..2 {
                    for c in 0..2 {
                        let v = h.get(i, r, c) + resp * sh[r * 2 + c] * 0.01;
                        h.set(i, r, c, v);
                    }
                }
            }
            for r in 0..2 {
                let v = h.get(i, r, r) + C64::new(0.002, 0.0);
                h.set(i, r, r, v);
            }
        }
        h
    }

    fn opts() -> WarpOptions {
        WarpOptions {
            band_lo_hz: 40.0,
            band_hi_hz: 2400.0,
            bank_hi_hz: 3000.0,
            delay_band_hz: (2500.0, 4000.0),
            noise_band_hz: (4000.0, 6000.0),
            ..WarpOptions::default()
        }
    }

    fn mask() -> Vec<bool> {
        let df = FS / N as f64;
        (0..=N / 2)
            .map(|i| (5.0..=2500.0).contains(&(i as f64 * df)))
            .collect()
    }

    /// Two pulses' worth of broadband drives through `truth`.
    fn pulses(truth: &MatrixStack, scale: f64) -> Vec<PulseSpectra> {
        (0..2)
            .map(|p| {
                let drives: Vec<Vec<C64>> = (0..2)
                    .map(|c| {
                        (0..=N / 2)
                            .map(|i| {
                                let ph = 0.37 * (i * (c + 1 + 3 * p)) as f64;
                                C64::new(ph.cos(), ph.sin())
                            })
                            .collect()
                    })
                    .collect();
                let responses = (0..2)
                    .map(|r| {
                        (0..=N / 2)
                            .map(|i| (0..2).map(|c| truth.get(i, r, c) * drives[c][i]).sum())
                            .collect()
                    })
                    .collect();
                PulseSpectra {
                    drives,
                    responses,
                    scale,
                }
            })
            .collect()
    }

    /// The fit finds a 6 % drop in the octave that holds the 100 Hz mode and
    /// none in the octave that holds the 900 Hz mode, and the plant the
    /// identification already is gives no drop anywhere.
    #[test]
    fn mimo_shock_r4_fit_finds_each_band_drop() {
        let id = plant(&[100.0, 900.0], &[0.0, 0.0]);
        let truth = plant(&[100.0, 900.0], &[0.06, 0.0]);
        let warp = WarpModel::new(id.clone(), mask(), FS, opts()).unwrap();
        let bands = Bands::octaves(&warp, &mask(), 1.0).unwrap();
        let reference: Vec<Vec<C64>> = vec![vec![C64::new(1.0, 0.0); N / 2 + 1]; 2];
        let fit = fit_softening(
            &warp,
            &bands,
            &pulses(&truth, 0.5),
            &reference,
            &FitOptions::default(),
        )
        .unwrap();
        let b100 = ((100.0f64 / 40.0).log2()).floor() as usize;
        let b900 = ((900.0f64 / 40.0).log2()).floor() as usize;
        assert!(
            (fit.drops[b100] - 0.06).abs() < 0.004,
            "100 Hz band: {:?}",
            fit.drops
        );
        assert!(
            fit.drops[b900].abs() < 0.002,
            "900 Hz band: {:?}",
            fit.drops
        );
        assert!(
            (fit.amplitude[b100] - 0.5 * fit.amplitude[b100] / 0.5).abs() < 1e-12
                && fit.amplitude[b100] > 0.0
        );
        let none = fit_softening(
            &warp,
            &bands,
            &pulses(&id, 0.5),
            &reference,
            &FitOptions::default(),
        )
        .unwrap();
        assert!(
            none.drops.iter().all(|d| d.abs() < 1e-12),
            "{:?}",
            none.drops
        );
    }

    #[test]
    fn mimo_shock_r4_extrapolate_drop_bounds_the_power() {
        // A drop growing as the amplitude: 1 % at 0.01, 2 % at 0.02, 8 % at 0.08.
        let d = extrapolate_drop(Some((0.01, 0.01)), 0.02, 0.02, 0.08, 1.0);
        assert!((d - 0.08).abs() < 1e-12);
        // Growing faster than the amplitude: held to the power 1.
        let d = extrapolate_drop(Some((0.01, 0.01)), 0.02, 0.04, 0.08, 1.0);
        assert!((d - 0.16).abs() < 1e-12);
        // Falling: power 0, the last drop kept.
        let d = extrapolate_drop(Some((0.01, 0.03)), 0.02, 0.02, 0.08, 1.0);
        assert!((d - 0.02).abs() < 1e-12);
        // No earlier point, or none there: the largest power.
        assert!((extrapolate_drop(None, 0.02, 0.02, 0.04, 0.5) - 0.02 * 2f64.sqrt()).abs() < 1e-12);
        assert!((extrapolate_drop(Some((0.01, 0.0)), 0.02, 0.02, 0.04, 1.0) - 0.04).abs() < 1e-12);
        // No drop: none; never carried downward.
        assert_eq!(
            extrapolate_drop(Some((0.01, 0.01)), 0.02, 0.0, 0.08, 1.0),
            0.0
        );
        assert_eq!(extrapolate_drop(None, 0.02, 0.03, 0.01, 1.0), 0.03);
    }

    fn observe(f: &mut SofteningFamily, level: f64, truth: &MatrixStack) {
        let s = level_scale(level);
        for p in pulses(truth, s).iter().take(1) {
            f.observe(&Observation {
                level_db: level,
                drives: &p.drives,
                responses: &p.responses,
            })
            .unwrap();
        }
    }

    /// Probes at two levels are played on the identification; the step to
    /// the first ladder level sets the family from the fits with the margin
    /// and the ceiling; `Probes` keeps it, `Levels` with `narrow` 0.5 never
    /// narrows by more than half a step.
    #[test]
    fn mimo_shock_r4_softening_family_sets_and_follows() {
        let id = plant(&[100.0, 900.0], &[0.0, 0.0]);
        let warp = WarpModel::new(id.clone(), mask(), FS, opts()).unwrap();
        let reference: Vec<Vec<C64>> = vec![vec![C64::new(1.0, 0.0); N / 2 + 1]; 2];
        let b100 = ((100.0f64 / 40.0).log2()).floor() as usize;
        let make = |follow| {
            SofteningFamily::new(
                warp.clone(),
                id.clone(),
                mask(),
                reference.clone(),
                SofteningPlan {
                    follow,
                    ceiling: 0.2,
                    ..SofteningPlan::default()
                },
            )
            .unwrap()
        };
        for follow in [
            Follow::Probes,
            Follow::Levels {
                narrow: 0.5,
                within_level: false,
            },
        ] {
            let mut f = make(follow);
            observe(&mut f, -40.0, &plant(&[100.0, 900.0], &[0.01, 0.0]));
            assert_eq!(f.model_for(-34.0).unwrap(), ModelSource::Initial);
            assert_eq!(f.current(), &id);
            observe(&mut f, -34.0, &plant(&[100.0, 900.0], &[0.02, 0.0]));
            assert_eq!(f.model_for(-12.0).unwrap(), ModelSource::Predicted);
            let a = f.assumed()[b100];
            // The fits' amplitudes are the scales here (the responses are not
            // the reference), so the extrapolated drop is checked against
            // the fit's own numbers.
            let l = f.levels();
            let want = (1.5
                * extrapolate_drop(
                    Some((l[0].fit.amplitude[b100], l[0].fit.drops[b100])),
                    l[1].fit.amplitude[b100],
                    l[1].fit.drops[b100],
                    level_scale(-12.0),
                    1.0,
                ))
            .min(0.2);
            assert!((a - want).abs() < 1e-12 && a > 0.0, "{a} {want}");
            assert_ne!(f.current(), &id);
            // A stiffer level next.
            observe(&mut f, -12.0, &plant(&[100.0, 900.0], &[0.0, 0.0]));
            f.model_for(-9.0).unwrap();
            let b = f.assumed()[b100];
            match follow {
                Follow::Probes => assert_eq!(a, b),
                _ => assert!((b - 0.5 * a).abs() < 1e-12, "{a} {b}"),
            }
        }
    }

    /// With a ladder start, a level a ladder guard inserts between the last
    /// probe and the first ladder level is a probe: the family is re-set (and
    /// only widened) on the step to it and again on the step to the ladder's
    /// first level, and kept from there. Without a ladder start it would be
    /// kept from the inserted level.
    #[test]
    fn mimo_shock_r4_probes_reach_the_ladder_start() {
        let id = plant(&[100.0, 900.0], &[0.0, 0.0]);
        let warp = WarpModel::new(id.clone(), mask(), FS, opts()).unwrap();
        let reference: Vec<Vec<C64>> = vec![vec![C64::new(1.0, 0.0); N / 2 + 1]; 2];
        let b100 = ((100.0f64 / 40.0).log2()).floor() as usize;
        let mut kept = Vec::new();
        for start in [Some(-12.0), None] {
            let mut f = SofteningFamily::new(
                warp.clone(),
                id.clone(),
                mask(),
                reference.clone(),
                SofteningPlan {
                    ladder_start_db: start,
                    ceiling: 0.9,
                    margin: 0.1,
                    ..SofteningPlan::default()
                },
            )
            .unwrap();
            observe(&mut f, -40.0, &plant(&[100.0, 900.0], &[0.005, 0.0]));
            f.model_for(-34.0).unwrap();
            observe(&mut f, -34.0, &plant(&[100.0, 900.0], &[0.01, 0.0]));
            f.model_for(-28.5).unwrap();
            let at_inserted = f.assumed()[b100];
            observe(&mut f, -28.5, &plant(&[100.0, 900.0], &[0.03, 0.0]));
            f.model_for(-12.0).unwrap();
            let at_ladder = f.assumed()[b100];
            observe(&mut f, -12.0, &plant(&[100.0, 900.0], &[0.0, 0.0]));
            f.model_for(-9.0).unwrap();
            assert_eq!(f.assumed()[b100], at_ladder);
            kept.push((at_inserted, at_ladder));
        }
        let (with, without) = (kept[0], kept[1]);
        assert!(with.0 > 0.0 && with.1 > with.0, "{with:?}");
        assert_eq!(without.0, without.1, "{without:?}");
    }

    /// A drop that grows as the square of the amplitude is under-predicted
    /// by a power of one: the measured margin rises by the ratio, and the
    /// next family is set with it; at one level the family narrows to the
    /// level's own fit and says so.
    #[test]
    fn mimo_shock_r4_margin_is_measured_and_the_level_narrows() {
        let id = plant(&[100.0, 900.0], &[0.0, 0.0]);
        let warp = WarpModel::new(id.clone(), mask(), FS, opts()).unwrap();
        // The reference is the plant's own full-level response, so a fit's
        // amplitude is its level's scale.
        let reference = pulses(&id, 1.0)[0].responses.clone();
        let b100 = ((100.0f64 / 40.0).log2()).floor() as usize;
        let mut f = SofteningFamily::new(
            warp,
            id.clone(),
            mask(),
            reference,
            SofteningPlan {
                probe_levels: 1,
                follow: Follow::Narrowing { narrow: 0.0 },
                data_margin: true,
                p_max: f64::INFINITY,
                ceiling: 1.0,
                ..SofteningPlan::default()
            },
        )
        .unwrap();
        let at = |d: f64| plant(&[100.0, 900.0], &[d, 0.0]);
        observe(&mut f, -40.0, &at(0.01));
        f.model_for(-34.0).unwrap();
        observe(&mut f, -34.0, &at(0.02));
        f.model_for(-28.0).unwrap();
        assert!(f.prediction_error().0 < 1.1, "{:?}", f.prediction_error());
        // Four times the drop for twice the amplitude: a power of two.
        observe(&mut f, -28.0, &at(0.08));
        let e = f.prediction_error().0;
        assert!(e > 1.5 && e < 2.5, "measured margin factor {e}");
        f.model_for(-26.0).unwrap();
        let wide = f.assumed()[b100];
        let l = f.levels();
        let p = (l[2].fit.drops[b100] / l[1].fit.drops[b100]).ln()
            / (l[2].fit.amplitude[b100] / l[1].fit.amplitude[b100]).ln();
        let want = (2.0 + p) / 2.0
            * e
            * extrapolate_drop(
                None,
                l[2].fit.amplitude[b100],
                l[2].fit.drops[b100],
                level_scale(-26.0),
                p,
            );
        assert!(
            want < 1.0 && (wide - want).abs() < 1e-9,
            "{wide} {want} {:?}",
            f.levels()
                .iter()
                .map(|l| (l.fit.amplitude[b100], l.fit.drops[b100]))
                .collect::<Vec<_>>()
        );
        // The plant at -26 dB is much stiffer than assumed: the level narrows.
        observe(&mut f, -26.0, &at(0.02));
        assert_eq!(f.model_for(-26.0).unwrap(), ModelSource::LevelOwn);
        assert!(f.assumed()[b100] < 0.5 * wide);
        // Nothing new at the level: kept, and not reported as the level's own.
        assert_eq!(f.model_for(-26.0).unwrap(), ModelSource::Predicted);
    }
}
