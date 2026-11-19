//! Law 2: the open-loop pseudo-inverse, re-applied every loop. The SDK's
//! worked example of a multiple-output law.
//!
//! Sources: D. O. Smallwood, "Multiple Shaker Random Vibration Control: An
//! Update", Proc. IEST 1999 (the drive `G_uu = H^+ G_ref H^+^H`, least squares
//! with more controls than drives, minimum norm with fewer); R. Schultz, "A
//! Demonstration of Force Estimation and Regularization Methods for
//! Multi-Shaker Testing", IMAC proceedings (truncated and Tikhonov
//! regularization).
//!
//! The drive depends only on the reference and the plant estimate. With the
//! FRF held it is the same every loop, so the run shows what the plant does
//! with the pseudo-inverse drive: with more controls than drives the response
//! settles at the projection `P G_ref P^H`, `P = H H^+`. With the FRF updated
//! each loop it follows the plant. With weighting on, the least-squares
//! inverse is `(W H)^+ W` with `W = diag(1 / sqrt(G_ref,kk))`.

use crate::factor::Step;
use crate::pinv::ALL_SHAPES;
use crate::spec::{LawSpec, ParamVal};
use crate::support::{
    band_hermitian, diag_condition, diag_correction, diag_kept, inverse_diagnostics,
    largest_change_db, regularization, regularization_params, weighting, weighting_param,
    InverseCache, InverseRule, ZERO,
};
use crate::BuiltinMimoLaw;
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};
use hertzforge_sdm::linalg;
use hertzforge_sdm::{HermitianStack, MatrixStack};

/// The settings of [`OpenLoopLaw`].
#[derive(Clone, Debug, PartialEq)]
pub struct OpenLoopConfig {
    /// `truncated`, `tikhonov` or `none`.
    pub regularization: String,
    /// The regularization's relative amount.
    pub regularization_amount: f64,
    /// `none` or `reference`.
    pub weighting: String,
}

impl Default for OpenLoopConfig {
    fn default() -> Self {
        OpenLoopConfig {
            regularization: "truncated".into(),
            regularization_amount: 1e-3,
            weighting: "none".into(),
        }
    }
}

impl OpenLoopConfig {
    /// Set the parameter `key`.
    pub fn set(&mut self, key: &str, v: &ParamVal) -> Result<(), LawError> {
        match key {
            "regularization" => self.regularization = v.choice(key)?.to_string(),
            "regularization_amount" => self.regularization_amount = v.number(key)?,
            "weighting" => self.weighting = v.choice(key)?.to_string(),
            other => return Err(LawError(format!("parameter `{other}` is not declared"))),
        }
        Ok(())
    }
}

/// The open-loop pseudo-inverse law for one run.
pub struct OpenLoopLaw {
    lines: usize,
    drives: usize,
    band: (usize, usize),
    /// The reference on the in-band lines.
    reference: HermitianStack,
    rule: InverseRule,
    cache: InverseCache,
    last: Option<HermitianStack>,
}

impl OpenLoopLaw {
    /// The law for `setup` with `cfg`.
    pub fn new(setup: &MimoSetup, cfg: &OpenLoopConfig) -> Result<Self, LawError> {
        setup.check()?;
        Ok(OpenLoopLaw {
            lines: setup.lines,
            drives: setup.drives,
            band: setup.band,
            reference: band_hermitian(&setup.reference, setup.band),
            rule: InverseRule::Regularized {
                reg: regularization(&cfg.regularization, cfg.regularization_amount)?,
                weighting: weighting(&cfg.weighting)?,
            },
            cache: InverseCache::default(),
            last: None,
        })
    }

    fn step(&mut self, frf: &MatrixStack) -> Result<Step, LawError> {
        let inv = self.cache.get(frf, self.band, &self.reference, self.rule)?;
        let inband = linalg::congruence(&inv.hp, &self.reference)?;
        let per = HermitianStack::packed_len(self.drives);
        let mut data = vec![ZERO; self.lines * per];
        data[self.band.0 * per..self.band.1 * per].copy_from_slice(inband.data());
        let drive = HermitianStack::from_packed(self.lines, self.drives, data)?;
        let (condition, kept) = inverse_diagnostics(inv, self.lines, self.band);
        let correction_db = self
            .last
            .as_ref()
            .map(|last| largest_change_db(last, &drive, self.band))
            .unwrap_or(0.0);
        self.last = Some(drive.clone());
        Ok(Step {
            drive,
            condition,
            kept,
            correction_db,
            clamped: 0,
        })
    }
}

impl MimoControlLaw for OpenLoopLaw {
    fn needs(&self) -> &[MimoInput] {
        &[]
    }

    fn shapes(&self) -> &[MimoShape] {
        &ALL_SHAPES
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        self.last = None;
        Ok(self.step(inputs.frf)?.outputs(false))
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        Ok(self.step(inputs.frf)?.outputs(false))
    }
}

impl BuiltinMimoLaw for OpenLoopLaw {
    fn spec() -> LawSpec {
        let mut params = regularization_params();
        params.push(weighting_param());
        LawSpec {
            name: "pseudo-inverse, open loop",
            version: "1.0.0",
            author: "Hertzforge",
            summary: "The regularized pseudo-inverse drive, H+ G_ref H+^H, computed again every \
                      loop from the plant estimate and nothing else: least squares with more \
                      controls than drives, minimum norm with fewer. With the FRF held it shows \
                      what the plant does with that drive; with the FRF updated it follows the \
                      plant.",
            params,
            diagnostics: vec![diag_condition(), diag_kept(), diag_correction()],
            shapes: ALL_SHAPES.to_vec(),
            needs: Vec::new(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        let mut cfg = OpenLoopConfig::default();
        for (k, v) in params {
            cfg.set(k, v)?;
        }
        Self::new(setup, &cfg)
    }
}
