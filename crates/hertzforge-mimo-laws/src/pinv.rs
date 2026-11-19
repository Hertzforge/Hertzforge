//! Law 1, the default: the regularized pseudo-inverse with closed-loop
//! correction of a factor of the reference.
//!
//! Source: D. O. Smallwood, "Multiple Shaker Random Vibration Control: An
//! Update", Proc. IEST 1999; regularization after R. Schultz, "A Demonstration
//! of Force Estimation and Regularization Methods for Multi-Shaker Testing",
//! IMAC proceedings. The loop itself is [`crate::factor`].
//!
//! The seed is the regularized pseudo-inverse drive `G_uu = H^+ G_ref H^+^H`.
//! Each loop then corrects a lower-triangular factor `Z` of the reference
//! (`Z Z^H` is the coupled-source matrix the inverse maps to the drives) from
//! the error between the reference and the measured response, so every drive
//! is positive semi-definite by construction.

use crate::factor::{FactorLoop, LoopSettings};
use crate::spec::{LawSpec, ParamVal};
use crate::support::{
    cap_params, convergence_param, diag_clamped, diag_condition, diag_correction, diag_kept,
    rank_tolerance_param, regularization, regularization_params, weighting, weighting_param,
    InverseRule,
};
use crate::BuiltinMimoLaw;
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};

pub(crate) const ALL_SHAPES: [MimoShape; 3] = [
    MimoShape::Square,
    MimoShape::MoreControlsThanDrives,
    MimoShape::FewerControlsThanDrives,
];

/// The settings of [`PseudoInverseLaw`], one per declared parameter.
#[derive(Clone, Debug, PartialEq)]
pub struct PseudoInverseConfig {
    /// `truncated`, `tikhonov` or `none`.
    pub regularization: String,
    /// The regularization's relative amount.
    pub regularization_amount: f64,
    /// The convergence factor `e`.
    pub convergence: f64,
    /// Largest rise per loop, dB.
    pub cap_up_db: f64,
    /// Largest fall per loop, dB.
    pub cap_down_db: f64,
    /// Rank tolerance.
    pub rank_tolerance: f64,
    /// `none` or `reference`.
    pub weighting: String,
}

impl Default for PseudoInverseConfig {
    fn default() -> Self {
        PseudoInverseConfig {
            regularization: "truncated".into(),
            regularization_amount: 1e-3,
            convergence: 0.5,
            cap_up_db: 3.5,
            cap_down_db: 6.0,
            rank_tolerance: 1e-6,
            weighting: "none".into(),
        }
    }
}

impl PseudoInverseConfig {
    /// Set the parameter `key`.
    pub fn set(&mut self, key: &str, v: &ParamVal) -> Result<(), LawError> {
        match key {
            "regularization" => self.regularization = v.choice(key)?.to_string(),
            "regularization_amount" => self.regularization_amount = v.number(key)?,
            "convergence" => self.convergence = v.number(key)?,
            "cap_up_db" => self.cap_up_db = v.number(key)?,
            "cap_down_db" => self.cap_down_db = v.number(key)?,
            "rank_tolerance" => self.rank_tolerance = v.number(key)?,
            "weighting" => self.weighting = v.choice(key)?.to_string(),
            other => return Err(LawError(format!("parameter `{other}` is not declared"))),
        }
        Ok(())
    }

    pub(crate) fn rule(&self) -> Result<InverseRule, LawError> {
        Ok(InverseRule::Regularized {
            reg: regularization(&self.regularization, self.regularization_amount)?,
            weighting: weighting(&self.weighting)?,
        })
    }

    pub(crate) fn settings(&self) -> LoopSettings {
        LoopSettings {
            convergence: self.convergence,
            cap_up_db: self.cap_up_db,
            cap_down_db: self.cap_down_db,
            rank_tol: self.rank_tolerance,
        }
    }
}

/// The pseudo-inverse law with closed-loop correction for one run.
pub struct PseudoInverseLaw {
    inner: FactorLoop,
}

impl PseudoInverseLaw {
    /// The law for `setup` with `cfg`.
    pub fn new(setup: &MimoSetup, cfg: &PseudoInverseConfig) -> Result<Self, LawError> {
        Ok(PseudoInverseLaw {
            inner: FactorLoop::new(setup, &setup.reference, cfg.rule()?, cfg.settings())?,
        })
    }
}

impl MimoControlLaw for PseudoInverseLaw {
    fn needs(&self) -> &[MimoInput] {
        &[]
    }

    fn shapes(&self) -> &[MimoShape] {
        &ALL_SHAPES
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        Ok(self.inner.seed(inputs.frf)?.outputs(true))
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        Ok(self
            .inner
            .iterate(inputs.frf, inputs.response)?
            .outputs(true))
    }
}

impl BuiltinMimoLaw for PseudoInverseLaw {
    fn spec() -> LawSpec {
        let mut params = regularization_params();
        params.push(convergence_param());
        params.extend(cap_params());
        params.push(rank_tolerance_param());
        params.push(weighting_param());
        LawSpec {
            name: "pseudo-inverse, closed loop",
            version: "1.0.0",
            author: "Hertzforge",
            summary: "Starts from the regularized pseudo-inverse drive, then each loop corrects \
                      a lower-triangular factor of the reference from the measured error \
                      (Smallwood's coupling-matrix correction), autospectra first in channel \
                      order, each factor diagonal and each drive PSD held to a per-loop limit, \
                      the sources held to the plant's rank. Every drive is positive \
                      semi-definite by construction.",
            params,
            diagnostics: vec![
                diag_condition(),
                diag_kept(),
                diag_correction(),
                diag_clamped(),
            ],
            shapes: ALL_SHAPES.to_vec(),
            needs: Vec::new(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        let mut cfg = PseudoInverseConfig::default();
        for (k, v) in params {
            cfg.set(k, v)?;
        }
        Self::new(setup, &cfg)
    }
}
