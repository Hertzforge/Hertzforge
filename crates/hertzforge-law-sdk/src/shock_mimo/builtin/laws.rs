//! The built-in laws as built from their declared parameters: one
//! constructor that the controller's native adapters and the law files both
//! call, so a parameter means the same mathematics on either side.
//!
//! The `model` parameter chooses how the law's plant model follows a plant
//! that changes with level ([`ModelChoice`]); the choice is read by name, so
//! a further follower is one more name here and one more arm in
//! [`ModelOptions::provider`], with no change to the law interface.

use super::super::math;
use crate::Params;
use math::inverse::{InverseExtras, InverseLaw, InverseParams};
use math::modal::{fit_modal, FitOptions};
use math::provider::{
    ModelProvider, MultiSecantModel, PoleTrackModel, SecantOptions, TrackOptions,
};
use math::shape::{CarryGain, ShapeExtras, ShapeLaw, ShapeParams, ShapeSafety};
use math::{drive_mask, level_scale, LawSteps, MatrixStack, PulseInputs, PulseOutputs};

/// How a built-in law's plant model follows the plant from pulse to pulse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelChoice {
    /// The identification, never changed.
    Frozen,
    /// Every pulse at the level being played is a constraint; the model is
    /// the least change of the level's starting model that reproduces all of
    /// them.
    Secant,
    /// A modal model is fitted to the identification once; each pulse
    /// re-fits its modes' frequencies and damping, and the first pulse of a
    /// new level gets them extrapolated along the level trend. When the
    /// tracked model predicts the pulses worse than `secant` does, `secant`'s
    /// model is used until it predicts better again.
    Track,
    /// `track`, with the pulses of the level being played reproduced exactly
    /// on top of the tracked model.
    TrackSecant,
}

impl ModelChoice {
    /// Every choice, by its parameter name.
    pub const ALL: [ModelChoice; 4] = [
        ModelChoice::Frozen,
        ModelChoice::Secant,
        ModelChoice::Track,
        ModelChoice::TrackSecant,
    ];

    /// The names the `model` parameter takes.
    pub const NAMES: &'static [&'static str] = &["frozen", "secant", "track", "track_secant"];

    /// The parameter name.
    pub fn name(self) -> &'static str {
        match self {
            ModelChoice::Frozen => "frozen",
            ModelChoice::Secant => "secant",
            ModelChoice::Track => "track",
            ModelChoice::TrackSecant => "track_secant",
        }
    }

    /// The choice named `name`.
    pub fn from_name(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|c| c.name() == name)
            .ok_or_else(|| {
                format!(
                    "the model `{name}` is not one of {}",
                    Self::NAMES.join(", ")
                )
            })
    }

    /// Whether the choice fits a modal model.
    pub fn is_modal(self) -> bool {
        matches!(self, ModelChoice::Track | ModelChoice::TrackSecant)
    }
}

/// The most modes a modal model is fitted with.
pub const MAX_MODES: usize = 24;

/// With the mode count left to the law, modes are added until the fit's
/// residual against the identification is under this.
pub const AUTO_FIT_RESIDUAL: f64 = 0.01;

/// With the mode count left to the law, at most this many fits follow the
/// first.
pub const AUTO_FIT_ROUNDS: usize = 4;

/// How a built-in law's model is built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelOptions {
    /// How the model follows the plant.
    pub model: ModelChoice,
    /// Modes in the modal model; zero counts the resonance peaks of the
    /// identified plant in the band ([`count_modes`]).
    pub modes: usize,
    /// Whether the identification is the first point of the level trend (it
    /// needs the identification level).
    pub from_identification: bool,
}

impl Default for ModelOptions {
    fn default() -> Self {
        ModelOptions {
            model: ModelChoice::Frozen,
            modes: 0,
            from_identification: false,
        }
    }
}

/// What a model build reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelBuild {
    /// Modes fitted (zero when no modal model was fitted).
    pub modes: usize,
    /// The modal fit's residual against the identification over the band,
    /// relative (NaN when no modal model was fitted).
    pub fit_residual: f64,
    /// Whether the fit was too far from the identification to track, so the
    /// run starts on the secant model.
    pub starts_on_fallback: bool,
}

impl ModelOptions {
    /// The provider for `setup` (`None` for the frozen model, which the laws
    /// keep themselves), and what the build found.
    pub fn provider(
        &self,
        setup: &math::Setup,
        identification_level_db: f64,
    ) -> Result<(Option<Box<dyn ModelProvider>>, ModelBuild), String> {
        let h = setup.plant.frf.clone();
        let band = drive_mask(setup);
        let none = ModelBuild {
            modes: 0,
            fit_residual: f64::NAN,
            starts_on_fallback: false,
        };
        let secant = || -> Box<dyn ModelProvider> {
            Box::new(MultiSecantModel::new(
                h.clone(),
                band.clone(),
                SecantOptions::default(),
            ))
        };
        match self.model {
            ModelChoice::Frozen => Ok((None, none)),
            ModelChoice::Secant => Ok((Some(secant()), none)),
            ModelChoice::Track | ModelChoice::TrackSecant => {
                if !band.iter().any(|&b| b) {
                    return Err(
                        "no line of the band is trusted, so no modal model can be fitted".into(),
                    );
                }
                let fit_with = |n_modes: usize| {
                    fit_modal(
                        &h,
                        setup.fs_hz,
                        &FitOptions {
                            band_lo_hz: setup.band_lo_hz,
                            band_hi_hz: setup.band_hi_hz,
                            n_modes,
                            ..FitOptions::default()
                        },
                    )
                };
                // A given count is fitted as given. Counted, the peaks are a
                // floor (close or weak modes hide): two more modes at a time
                // until the fit is within AUTO_FIT_RESIDUAL of the
                // identification, AUTO_FIT_ROUNDS more fits at most.
                let (modes, fit) = if self.modes == 0 {
                    let mut m = count_modes(&h, &band);
                    let mut fit = fit_with(m)?;
                    let mut rounds = 0;
                    while (fit.residual_rel.is_nan() || fit.residual_rel > AUTO_FIT_RESIDUAL)
                        && rounds < AUTO_FIT_ROUNDS
                        && m + 2 <= MAX_MODES
                    {
                        m += 2;
                        rounds += 1;
                        let next = fit_with(m)?;
                        if next.residual_rel.is_nan() || next.residual_rel >= fit.residual_rel {
                            m -= 2;
                            break;
                        }
                        fit = next;
                    }
                    (m, fit)
                } else {
                    let m = self.modes.min(MAX_MODES);
                    (m, fit_with(m)?)
                };
                let opts = TrackOptions {
                    secant: (self.model == ModelChoice::TrackSecant).then(SecantOptions::default),
                    identification_scale: (self.from_identification
                        && identification_level_db.is_finite())
                    .then(|| level_scale(identification_level_db)),
                    ..TrackOptions::default()
                };
                let starts_on_fallback =
                    fit.residual_rel.is_nan() || fit.residual_rel > opts.seed_residual_max;
                let p = PoleTrackModel::new(
                    fit.model,
                    h.clone(),
                    band.clone(),
                    setup.fs_hz,
                    opts,
                    secant(),
                )?;
                Ok((
                    Some(Box::new(p)),
                    ModelBuild {
                        modes,
                        fit_residual: fit.residual_rel,
                        starts_on_fallback,
                    },
                ))
            }
        }
    }
}

/// The number of resonances the identified plant shows in the band: peaks of
/// `sum |H_ij|^2` (the squared Frobenius norm per line) that are the largest
/// within 4% of their frequency either side and stand at least 3 dB over
/// the lower of the two valleys between them and the next larger value on
/// each side. At least one, at most [`MAX_MODES`].
pub fn count_modes(h: &MatrixStack, band: &[bool]) -> usize {
    let lines = h.lines();
    let t: Vec<f64> = (0..lines)
        .map(|f| h.line(f).iter().map(|v| v.norm_sqr()).sum::<f64>())
        .collect();
    let idx: Vec<usize> = (1..lines).filter(|&f| band[f] && t[f] > 0.0).collect();
    let mut peaks = 0usize;
    for (i, &f) in idx.iter().enumerate() {
        let lo = (f as f64 * 0.96).floor() as usize;
        let hi = (f as f64 * 1.04).ceil() as usize;
        let local_max = idx
            .iter()
            .filter(|&&g| g != f && g >= lo && g <= hi)
            .all(|&g| t[g] < t[f]);
        if !local_max {
            continue;
        }
        // The valley on each side: the least value walking out until a
        // larger value or the band's edge.
        let valley = |range: &mut dyn Iterator<Item = &usize>| -> f64 {
            let mut v = t[f];
            for &g in range {
                if t[g] > t[f] {
                    break;
                }
                v = v.min(t[g]);
            }
            v
        };
        let left = valley(&mut idx[..i].iter().rev());
        let right = valley(&mut idx[i + 1..].iter());
        if t[f] >= 10f64.powf(0.3) * left.max(right) {
            peaks += 1;
        }
    }
    peaks.clamp(1, MAX_MODES)
}

/// A built-in law with its reported values kept to the ones it declares.
pub struct BuiltLaw {
    law: Box<dyn LawSteps + Send>,
    values: &'static [&'static str],
    build: ModelBuild,
}

impl BuiltLaw {
    /// What the model build found.
    pub fn model_build(&self) -> ModelBuild {
        self.build
    }

    fn keep(&self, mut o: PulseOutputs) -> PulseOutputs {
        o.values.retain(|(k, _)| self.values.contains(k));
        o
    }
}

impl LawSteps for BuiltLaw {
    fn seed(&mut self, level_db: f64) -> Result<PulseOutputs, String> {
        let o = self.law.seed(level_db)?;
        Ok(self.keep(o))
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String> {
        let o = self.law.iterate(inputs)?;
        Ok(self.keep(o))
    }
}

/// `inverse` as its parameters build it.
#[derive(Debug, Clone, PartialEq)]
pub struct InverseConfig {
    /// The law's own parameters.
    pub params: InverseParams,
    /// The model.
    pub model: ModelOptions,
    /// At a level step, correct the error the next level is predicted to have
    /// (needs a tracked model).
    pub predicted_step: bool,
}

impl Default for InverseConfig {
    fn default() -> Self {
        let d = super::defaults(&super::INVERSE);
        inverse_config(&d).expect("the declared defaults are valid")
    }
}

impl InverseConfig {
    /// The law for `setup`, its plant identified at `identification_level_db`.
    pub fn build(
        &self,
        setup: math::Setup,
        identification_level_db: f64,
    ) -> Result<BuiltLaw, String> {
        if self.predicted_step && !self.model.model.is_modal() {
            return Err(
                "the predicted level step needs a tracked model (`track` or \
                        `track_secant`)"
                    .into(),
            );
        }
        let (provider, build) = self.model.provider(&setup, identification_level_db)?;
        let law = InverseLaw::with_options(
            setup,
            self.params.clone(),
            InverseExtras {
                provider,
                robust_first: None,
                predicted_step: self.predicted_step,
            },
        )?;
        Ok(BuiltLaw {
            law: Box::new(law),
            values: INVERSE_VALUES,
            build,
        })
    }
}

/// The values `inverse` reports, in its declaration's order.
pub const INVERSE_VALUES: &[&str] = &[
    "waveform_error",
    "drive_rms_full_level",
    "least_line_gain",
    "lines_cut",
    "lines_refreshed",
    "model_source",
    "model_prediction_residual",
    "predicted_step",
];

/// `shape` as its parameters build it.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapeConfig {
    /// The law's own parameters.
    pub params: ShapeParams,
    /// The model.
    pub model: ModelOptions,
    /// Each accumulated gain stays within this many dB of one; zero: no
    /// clamp.
    pub gain_clamp_db: f64,
    /// A point whose error is above this and has not fallen in two pulses at
    /// a level, or whose gain is at the clamp, is reported stalled, dB.
    pub stall_tol_db: f64,
}

impl Default for ShapeConfig {
    fn default() -> Self {
        let d = super::defaults(&super::SHAPE);
        shape_config(&d).expect("the declared defaults are valid")
    }
}

impl ShapeConfig {
    /// The law for `setup`, its plant identified at `identification_level_db`.
    pub fn build(
        &self,
        setup: math::Setup,
        identification_level_db: f64,
    ) -> Result<BuiltLaw, String> {
        let (provider, build) = self.model.provider(&setup, identification_level_db)?;
        let law = ShapeLaw::with_options(
            setup,
            self.params.clone(),
            ShapeExtras {
                provider,
                safety: ShapeSafety {
                    gain_clamp_db: (self.gain_clamp_db > 0.0).then_some(self.gain_clamp_db),
                    stall_tol_db: self.stall_tol_db,
                    clip_tol: None,
                    carry: CarryGain::Keep,
                },
                robust_first: None,
            },
        )?;
        Ok(BuiltLaw {
            law: Box::new(law),
            values: SHAPE_VALUES,
            build,
        })
    }
}

/// The values `shape` reports, in its declaration's order.
pub const SHAPE_VALUES: &[&str] = &[
    "worst_srs_error_db",
    "weight",
    "drive_rms_full_level",
    "effort_cost_seed",
    "gain_extent_db",
    "stalled_points",
    "model_source",
    "model_prediction_residual",
];

/// The model options from checked values.
pub fn model_options(p: &Params) -> Result<ModelOptions, String> {
    let modes = p.integer("modes")?;
    Ok(ModelOptions {
        model: ModelChoice::from_name(p.choice("model")?)?,
        modes: usize::try_from(modes).map_err(|_| format!("{modes} modes"))?,
        from_identification: p.toggle("track_from_identification")?,
    })
}

/// `inverse` from checked values.
pub fn inverse_config(p: &Params) -> Result<InverseConfig, String> {
    Ok(InverseConfig {
        params: super::inverse_params(p)?,
        model: model_options(p)?,
        predicted_step: p.toggle("predicted_step")?,
    })
}

/// `shape` from checked values.
pub fn shape_config(p: &Params) -> Result<ShapeConfig, String> {
    Ok(ShapeConfig {
        params: super::shape_params(p)?,
        model: model_options(p)?,
        gain_clamp_db: p.number("gain_clamp_db")?,
        stall_tol_db: p.number("stall_tol_db")?,
    })
}
