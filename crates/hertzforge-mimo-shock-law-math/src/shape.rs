//! `shape`: SRS control by shaping each control's target, with no waveform
//! error formed anywhere.
//!
//! The law keeps a real, positive gain `C_c(f)` per control and a delay
//! `tau_c` per control. After a pulse, with the measured SRS `S_c` and the
//! target `T_c` at the SRS's natural frequencies:
//!
//! `C_c(f) <- C_c(f) * interp_f( clamp((T_c / S_c)^w) )`
//!
//! where the ratio is taken in dB, limited to `+/- max_step_db` a pulse, and
//! carried to the lines by linear interpolation in log frequency; `w` is the
//! weight for the level just played, from a table (`Derivations.md` D6). The
//! next drive is
//!
//! `D = s+ G (C o R(tau))`,   `R_c(f; tau) = R_c(f) exp(-j w tau_c)`
//!
//! with `G` the model's regularized inverse. The delays are re-estimated from
//! the measured response by cross-correlation against the shaped target and
//! relaxed: `tau_c <- tau_c + relax * d_c`, referred to control 0 and bounded.
//!
//! Options:
//! - `anchor = Measured` (requires `refresh`): the wanted response of the next
//!   pulse is the measured response with its own phase and its magnitude
//!   scaled by this pulse's ratio, and the drive moves by the model's inverse
//!   of the change: `D+ = D + G ((r - 1) o Y)` (`Derivations.md` D2, ledger
//!   MS-C1 with MS-C3).
//! - `refresh`: the model is moved along the drive after each pulse.
//! - `seed_timing = LeastEffort`: the delays before the first pulse are chosen
//!   for least unreachable energy plus drive ([`crate::delay`]).

use crate::delay::{apply_delay, estimate_delay, least_effort_delays, EffortCost, LeastEffort};
use crate::model::regularized_inverse;
use crate::provider::{ModelProvider, ModelSource, Observation};
use crate::refresh::refresh_along_drive;
use crate::robust::weighted_inverse;
use crate::spectrum::{apply, omegas, rms_all, rows_irfft, rows_rfft};
use crate::srs::{srs_rows, to_lines_log, weight_at, worst_error_db};
use crate::{
    check_inputs, check_setup, drive_mask, level_scale, LawSteps, MatrixStack, PulseInputs,
    PulseOutputs, Setup, C64,
};

/// What the SRS correction is applied to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// The target waveform, shaped by the accumulated gain (the default).
    Target,
    /// The measured response, keeping its phase (requires the refresh).
    Measured,
}

/// How the delays are set before the first pulse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeedTiming {
    /// Every control's target at zero delay.
    Zero,
    /// The least-effort delays.
    LeastEffort(LeastEffort),
}

/// Parameters of `shape`.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeParams {
    /// Tikhonov regularization relative to each line's largest singular value.
    pub lambda_rel: f64,
    /// The weight by level: `(level_db, weight)` rows, sorted by level.
    pub weights: Vec<(f64, f64)>,
    /// The largest change of a control's gain in one pulse, dB.
    pub max_step_db: f64,
    /// Relaxation of the delay re-estimation; zero turns it off.
    pub delay_relax: f64,
    /// Each delay stays inside `+/- delay_bound_s` of control 0's, seconds.
    pub delay_bound_s: f64,
    /// What the correction is applied to.
    pub anchor: Anchor,
    /// Whether the model is refreshed along the drive after each pulse.
    pub refresh: bool,
    /// The refresh blend, `(0, 1]`.
    pub refresh_mu: f64,
    /// The refresh's drive-energy floor relative to the largest line's.
    pub refresh_floor_rel: f64,
    /// How the delays are set before the first pulse.
    pub seed_timing: SeedTiming,
}

/// The default weight table: 0.5 at -12 dB rising to 0.85 at -3 dB and above
/// (the published single-drive schedule, `Derivations.md` D6).
pub const DEFAULT_WEIGHTS: [(f64, f64); 5] = [
    (-12.0, 0.5),
    (-9.0, 0.72),
    (-6.0, 0.82),
    (-3.0, 0.85),
    (0.0, 0.85),
];

impl Default for ShapeParams {
    fn default() -> Self {
        ShapeParams {
            lambda_rel: 1.0e-3,
            weights: DEFAULT_WEIGHTS.to_vec(),
            max_step_db: 6.0,
            delay_relax: 0.5,
            delay_bound_s: 2.0e-3,
            anchor: Anchor::Target,
            refresh: false,
            refresh_mu: 1.0,
            refresh_floor_rel: 1.0e-6,
            seed_timing: SeedTiming::Zero,
        }
    }
}

/// How the accumulated gains are carried through a change of model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CarryGain {
    /// The gains are kept whatever the model.
    Keep,
    /// The gains restart at one when the new model was learned at the level
    /// about to be played (it should need no shaping there).
    ResetOnOwn,
}

/// The safety options of `shape` built with [`ShapeLaw::with_options`]
/// (`Derivations.md` D15).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeSafety {
    /// Each accumulated gain stays within `+/-` this many dB; `None`: no clamp.
    pub gain_clamp_db: Option<f64>,
    /// A point is stalled when its gain is at the clamp, or when its error is
    /// above this many dB and has not fallen in two pulses at one level.
    pub stall_tol_db: f64,
    /// When the played drive delivered less than `1 - clip_tol` of what the
    /// law asked for (in phase, energy-weighted over a point's band), that
    /// point's gain is not updated. `None`: always updated.
    pub clip_tol: Option<f64>,
    /// How the gains pass through a model change.
    pub carry: CarryGain,
}

impl ShapeSafety {
    /// No safety change: the law as built with [`ShapeLaw::new`].
    pub fn off() -> Self {
        ShapeSafety {
            gain_clamp_db: None,
            stall_tol_db: 1.5,
            clip_tol: None,
            carry: CarryGain::Keep,
        }
    }

    /// The values the research record supports: a 12 dB clamp, the stall
    /// flag at 1.5 dB, no learning where a drive delivered under 90%.
    pub fn recommended() -> Self {
        ShapeSafety {
            gain_clamp_db: Some(12.0),
            stall_tol_db: 1.5,
            clip_tol: Some(0.1),
            carry: CarryGain::Keep,
        }
    }
}

/// What [`ShapeLaw::with_options`] adds to the law.
pub struct ShapeExtras {
    /// The model provider; `None` keeps the model as `params` says.
    pub provider: Option<Box<dyn ModelProvider>>,
    /// The safety options.
    pub safety: ShapeSafety,
    /// When set, the first pulse of each new level uses the
    /// uncertainty-weighted inverse with these mode-frequency drops
    /// (fractions) around the provider's model (it needs a provider with a
    /// modal model).
    pub robust_first: Option<Vec<f64>>,
}

impl Default for ShapeExtras {
    fn default() -> Self {
        ShapeExtras {
            provider: None,
            safety: ShapeSafety::off(),
            robust_first: None,
        }
    }
}

struct ShapeState {
    extras: ShapeExtras,
    stalled: Vec<Vec<bool>>,
    history: Vec<(f64, Vec<Vec<f64>>)>,
    last_drive: Option<Vec<Vec<C64>>>,
}

/// The `shape` law's state.
pub struct ShapeLaw {
    setup: Setup,
    params: ShapeParams,
    mask: Vec<bool>,
    omegas: Vec<f64>,
    model: MatrixStack,
    inverse: MatrixStack,
    reference: Vec<Vec<C64>>,
    gains: Vec<Vec<f64>>,
    delays: Vec<f64>,
    state: Option<ShapeState>,
}

impl ShapeLaw {
    /// The law for an SRS setup.
    pub fn new(setup: Setup, params: ShapeParams) -> Result<Self, String> {
        let lines = check_setup(&setup)?;
        if setup.srs.is_none() {
            return Err("shape controls an SRS test; the setup has no SRS target".into());
        }
        if params.anchor == Anchor::Measured && !params.refresh {
            return Err("the measured anchor needs the model refresh".into());
        }
        if params.max_step_db.is_nan()
            || params.max_step_db <= 0.0
            || params.delay_bound_s.is_nan()
            || params.delay_bound_s < 0.0
        {
            return Err("the step limit must be positive and the delay bound not negative".into());
        }
        if !(0.0..=1.0).contains(&params.delay_relax) {
            return Err("the delay relaxation must be in [0, 1]".into());
        }
        let mask = drive_mask(&setup);
        let model = setup.plant.frf.clone();
        let inverse = regularized_inverse(&model, &mask, params.lambda_rel)?;
        let reference = rows_rfft(&setup.reference, 1.0)?;
        Ok(ShapeLaw {
            omegas: omegas(setup.fs_hz, setup.record_len),
            gains: vec![vec![1.0; lines]; setup.n_controls],
            delays: vec![0.0; setup.n_controls],
            setup,
            params,
            mask,
            model,
            inverse,
            reference,
            state: None,
        })
    }

    /// The law with a model provider and the safety options. With
    /// `ShapeExtras::default()` it is the law of [`ShapeLaw::new`] with more
    /// diagnostics.
    pub fn with_options(
        setup: Setup,
        params: ShapeParams,
        extras: ShapeExtras,
    ) -> Result<Self, String> {
        if extras.provider.is_some() && params.refresh {
            return Err("a model provider replaces the refresh; turn the refresh off".into());
        }
        if extras.robust_first.is_some() && extras.provider.is_none() {
            return Err("the weighted first pulse needs a model provider".into());
        }
        if let Some(c) = extras.safety.gain_clamp_db {
            if c.is_nan() || c <= 0.0 {
                return Err("the gain clamp must be positive".into());
            }
        }
        let mut law = ShapeLaw::new(setup, params)?;
        if let Some(p) = &extras.provider {
            if p.current().dims() != law.model.dims() {
                return Err("the provider's model does not match the setup".into());
            }
            law.model = p.current().clone();
            law.inverse = regularized_inverse(&law.model, &law.mask, law.params.lambda_rel)?;
        }
        let n_pts = law
            .setup
            .srs
            .as_ref()
            .map(|t| t.natural_hz.len())
            .unwrap_or(0);
        law.state = Some(ShapeState {
            extras,
            stalled: vec![vec![false; n_pts]; law.setup.n_controls],
            history: Vec::new(),
            last_drive: None,
        });
        Ok(law)
    }

    /// Per control and SRS point, whether the stall diagnostic is raised (empty
    /// for a law built with [`ShapeLaw::new`]).
    pub fn stalled(&self) -> Vec<Vec<bool>> {
        self.state
            .as_ref()
            .map(|s| s.stalled.clone())
            .unwrap_or_default()
    }

    /// The largest `abs(20 log10 C)` over every control and driven line, dB.
    pub fn gain_extent_db(&self) -> f64 {
        let mut m = 0.0f64;
        for g in &self.gains {
            for (v, &on) in g.iter().zip(&self.mask) {
                if on && *v > 0.0 {
                    m = m.max((20.0 * v.log10()).abs());
                }
            }
        }
        m
    }

    /// The shaped targets at the current gains and delays, full level.
    fn shaped(&self) -> Vec<Vec<C64>> {
        self.reference
            .iter()
            .zip(&self.gains)
            .zip(&self.delays)
            .map(|((r, g), &tau)| {
                apply_delay(r, &self.omegas, tau)
                    .into_iter()
                    .zip(g)
                    .map(|(v, gv)| v * *gv)
                    .collect()
            })
            .collect()
    }

    /// The delays the law builds for now, seconds.
    pub fn delays(&self) -> &[f64] {
        &self.delays
    }
}

impl LawSteps for ShapeLaw {
    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, String> {
        let mut values = Vec::new();
        if let SeedTiming::LeastEffort(le) = self.params.seed_timing {
            let cost = EffortCost::new(
                &self.model,
                &self.inverse,
                &self.mask,
                &self.reference,
                &self.omegas,
                le.lambda,
            );
            let before = cost.cost(&vec![0.0; self.delays.len()]);
            self.delays = least_effort_delays(&cost, le);
            values.push(("effort_cost_zero_delay", before));
            values.push(("effort_cost_seed", cost.cost(&self.delays)));
        }
        let d = apply(&self.inverse, &self.shaped());
        let s = level_scale(level_db);
        let drives = rows_irfft(&d, self.setup.record_len, s)?;
        values.push(("drive_rms_full_level", rms_all(&drives) / s));
        if let Some(st) = self.state.as_mut() {
            st.last_drive = Some(d);
        }
        Ok(PulseOutputs {
            drives,
            delays_s: Some(self.delays.clone()),
            values,
        })
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String> {
        check_inputs(&self.setup, inputs)?;
        let target = self.setup.srs.as_ref().expect("checked at construction");
        let n = self.setup.record_len;
        let lines = n / 2 + 1;
        let df = self.setup.fs_hz / n as f64;
        let s = level_scale(inputs.level_db);
        let s_next = level_scale(inputs.next_level_db);

        let measured = match &inputs.srs {
            Some(m) => {
                if m.len() != self.setup.n_controls
                    || m.iter().any(|r| r.len() != target.natural_hz.len())
                {
                    return Err("the measured SRS must have one row per control".into());
                }
                m.iter()
                    .map(|r| r.iter().map(|v| v / s).collect())
                    .collect()
            }
            None => srs_rows(
                &inputs.responses,
                self.setup.fs_hz,
                &target.natural_hz,
                target.q,
                s,
            ),
        };
        let w = weight_at(&self.params.weights, inputs.level_db);
        let lim = self.params.max_step_db;
        // This pulse's ratio per control, at the SRS points, in dB.
        let mut db_points: Vec<Vec<f64>> = measured
            .iter()
            .zip(&target.points)
            .map(|(m, t)| {
                m.iter()
                    .zip(t)
                    .map(|(&mv, &tv)| {
                        let e = if mv > 0.0 && tv > 0.0 {
                            20.0 * (tv / mv).log10()
                        } else {
                            0.0
                        };
                        (w * e).clamp(-lim, lim)
                    })
                    .collect()
            })
            .collect();

        let d = rows_rfft(&inputs.drives, s)?;
        let y = rows_rfft(&inputs.responses, s)?;
        let mut extra_values: Vec<(&'static str, f64)> = Vec::new();
        if let Some(st) = self.state.as_mut() {
            // Points whose band the played drive could not deliver learn
            // nothing from this pulse.
            if let (Some(tol), Some(req)) = (st.extras.safety.clip_tol, &st.last_drive) {
                let frozen = clipped_points(&d, req, &target.natural_hz, df, tol);
                for row in db_points.iter_mut() {
                    for (v, &fz) in row.iter_mut().zip(&frozen) {
                        if fz {
                            *v = 0.0;
                        }
                    }
                }
                extra_values.push((
                    "points_frozen_by_clip",
                    frozen.iter().filter(|&&f| f).count() as f64,
                ));
            }
        }
        // ... carried to the lines.
        let ratios: Vec<Vec<f64>> = db_points
            .iter()
            .map(|db| {
                let at = to_lines_log(&target.natural_hz, db, lines, df);
                at.iter().map(|v| 10f64.powf(v / 20.0)).collect()
            })
            .collect();

        let mut first_of_level_inverse: Option<MatrixStack> = None;
        if let Some(st) = self.state.as_mut() {
            if let Some(p) = st.extras.provider.as_mut() {
                p.observe(&Observation {
                    level_db: inputs.level_db,
                    drives: &d,
                    responses: &y,
                })?;
                let source = p.model_for(inputs.next_level_db)?;
                self.model = p.current().clone();
                self.inverse =
                    regularized_inverse(&self.model, &self.mask, self.params.lambda_rel)?;
                if st.extras.safety.carry == CarryGain::ResetOnOwn
                    && source == ModelSource::LevelOwn
                {
                    for g in self.gains.iter_mut() {
                        g.iter_mut().for_each(|v| *v = 1.0);
                    }
                }
                if let Some(drops) = &st.extras.robust_first {
                    if inputs.next_level_db != inputs.level_db {
                        if let Some(rad) = p.radius(inputs.next_level_db, drops) {
                            first_of_level_inverse = Some(weighted_inverse(
                                &self.model,
                                &self.mask,
                                self.params.lambda_rel,
                                &rad,
                            )?);
                        }
                    }
                }
                let rep = p.report();
                extra_values.push(("model_prediction_residual", rep.prediction_residual));
                extra_values.push(("model_fit_residual", rep.fit_residual));
                extra_values.push(("model_fallback_residual", rep.fallback_residual));
                extra_values.push(("model_gate_fired", f64::from(u8::from(rep.gate_fired))));
                extra_values.push(("model_source", rep.source.code()));
            }
        }
        let mut refreshed = 0usize;
        if self.params.refresh {
            refreshed = refresh_along_drive(
                &mut self.model,
                &d,
                &y,
                self.params.refresh_mu,
                self.params.refresh_floor_rel,
            )?;
            self.inverse = regularized_inverse(&self.model, &self.mask, self.params.lambda_rel)?;
        }

        let next = match self.params.anchor {
            Anchor::Target => {
                for (g, r) in self.gains.iter_mut().zip(&ratios) {
                    for (gv, rv) in g.iter_mut().zip(r) {
                        *gv *= rv;
                    }
                }
                if let Some(c) = self
                    .state
                    .as_ref()
                    .and_then(|st| st.extras.safety.gain_clamp_db)
                {
                    let (lo, hi) = (10f64.powf(-c / 20.0), 10f64.powf(c / 20.0));
                    for g in self.gains.iter_mut() {
                        g.iter_mut().for_each(|v| *v = v.clamp(lo, hi));
                    }
                }
                if self.params.delay_relax > 0.0 {
                    let shaped = self.shaped();
                    let shaped_t = rows_irfft(&shaped, n, 1.0)?;
                    for ((resp, shaped_c), tau) in inputs
                        .responses
                        .iter()
                        .zip(&shaped_t)
                        .zip(self.delays.iter_mut())
                    {
                        let yt: Vec<f64> = resp.iter().map(|v| v / s).collect();
                        let dc = estimate_delay(&yt, shaped_c, self.setup.fs_hz)?;
                        *tau += self.params.delay_relax * dc;
                    }
                    let d0 = self.delays[0];
                    let b = self.params.delay_bound_s;
                    for tau in self.delays.iter_mut() {
                        *tau = (*tau - d0).clamp(-b, b);
                    }
                }
                apply(
                    first_of_level_inverse.as_ref().unwrap_or(&self.inverse),
                    &self.shaped(),
                )
            }
            Anchor::Measured => {
                let change: Vec<Vec<C64>> = y
                    .iter()
                    .zip(&ratios)
                    .map(|(yc, r)| yc.iter().zip(r).map(|(v, rv)| v * (rv - 1.0)).collect())
                    .collect();
                let step = apply(&self.inverse, &change);
                d.iter()
                    .zip(&step)
                    .map(|(dc, sc)| dc.iter().zip(sc).map(|(a, b)| a + b).collect())
                    .collect()
            }
        };
        let drives = rows_irfft(&next, n, s_next)?;
        let mut values = vec![
            (
                "worst_srs_error_db",
                worst_error_db(&measured, &target.points),
            ),
            ("weight", w),
            ("drive_rms_full_level", rms_all(&drives) / s_next),
            ("lines_refreshed", refreshed as f64),
        ];
        if self.state.is_some() {
            let extent = self.gain_extent_db();
            let clamp_db = self
                .state
                .as_ref()
                .and_then(|st| st.extras.safety.gain_clamp_db);
            let at_clamp: Vec<Vec<bool>> = self
                .gains
                .iter()
                .map(|g| {
                    target
                        .natural_hz
                        .iter()
                        .map(|&fhz| {
                            let k = ((fhz / df).round() as usize).min(g.len() - 1);
                            clamp_db
                                .map(|c| (20.0 * g[k].log10()).abs() >= c - 1e-9)
                                .unwrap_or(false)
                        })
                        .collect()
                })
                .collect();
            let errors: Vec<Vec<f64>> = measured
                .iter()
                .zip(&target.points)
                .map(|(m, t)| crate::srs::error_db(m, t))
                .collect();
            let Some(st) = self.state.as_mut() else {
                unreachable!("the state was checked above")
            };
            st.history.push((inputs.level_db, errors.clone()));
            let tol = st.extras.safety.stall_tol_db;
            let hl = st.history.len();
            for c in 0..errors.len() {
                for p in 0..errors[c].len() {
                    let mut stalled = at_clamp[c][p];
                    if hl >= 3 && st.history[hl - 3].0 == inputs.level_db {
                        let now = errors[c][p].abs();
                        let then = st.history[hl - 3].1[c][p].abs();
                        if now > tol && now >= then - 0.25 {
                            stalled = true;
                        }
                    }
                    st.stalled[c][p] = stalled;
                }
            }
            let n_stalled = st.stalled.iter().flatten().filter(|&&b| b).count();
            st.last_drive = Some(next);
            values.push(("gain_extent_db", extent));
            values.push(("stalled_points", n_stalled as f64));
            values.extend(extra_values);
        }
        Ok(PulseOutputs {
            values,
            drives,
            delays_s: Some(self.delays.clone()),
        })
    }
}

/// Per SRS point, whether the played drive delivered less than `1 - tol` of
/// the requested drive's energy in phase over the point's band (one
/// twenty-fourth of an octave either side), every drive together.
fn clipped_points(
    played: &[Vec<C64>],
    requested: &[Vec<C64>],
    natural_hz: &[f64],
    df: f64,
    tol: f64,
) -> Vec<bool> {
    let lines = played.first().map(|v| v.len()).unwrap_or(0);
    let h = 2f64.powf(1.0 / 24.0);
    natural_hz
        .iter()
        .map(|&fc| {
            let lo = ((fc / h / df).ceil() as usize).min(lines);
            let hi = ((fc * h / df).floor() as usize + 1).min(lines);
            let (mut num, mut den) = (0.0, 0.0);
            for (p, r) in played.iter().zip(requested) {
                for k in lo..hi {
                    num += (p[k] * r[k].conj()).re;
                    den += r[k].norm_sqr();
                }
            }
            den > 0.0 && num / den < 1.0 - tol
        })
        .collect()
}
