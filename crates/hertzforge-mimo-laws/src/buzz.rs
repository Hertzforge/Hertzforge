//! Law 4: buzz-test matched control. The reference keeps its specified
//! autospectra and takes every pair's coherence and phase from the
//! identification response: what the structure does on its own under
//! broadband, uncorrelated drive, which it can always produce.
//!
//! Origin: P. M. Daborn, C. Roberts, D. J. Ewins and P. R. Ind,
//! "Next-Generation Random Vibration Tests", IMAC XXXII (2014), and Daborn's
//! thesis "Smarter Dynamic Testing of Critical Structures", University of
//! Bristol (2014), neither read here; the method as stated by R. Schultz, "A
//! Demonstration of Force Estimation and Regularization Methods for
//! Multi-Shaker Testing", IMAC proceedings, who credits Daborn.
//!
//! With `B` the identification response on a line, the coherency is
//! `C_jk = B_jk / sqrt(B_jj B_kk)` and the modified reference is
//! `G_jk = sqrt(G_ref,jj G_ref,kk) C_jk` (`G = D C D`, `D = diag(sqrt(G_ref,jj))`).
//! A pair whose identification coherence `|C_jk|^2` is below the floor is left
//! independent (`G_jk = 0`). Zeroing pairs can leave the matrix indefinite, so
//! each line is then made positive semi-definite by clamping negative
//! eigenvalues and restoring the specified autospectra by a diagonal
//! congruence (the repair of N. Hunter, K. Cross and G. Nelson, "The Cross
//! Spectrum in Multiple Input Multiple Response Vibration Testing", IMAC
//! proceedings, with the diagonal restored). The closed loop is then the
//! default law's ([`crate::factor`]) on the modified reference: the brief's
//! proposal of a fixed buzz reference servoed in full.

use crate::factor::{FactorLoop, LoopSettings};
use crate::pinv::ALL_SHAPES;
use crate::spec::{LawSpec, ParamDecl, ParamKindDecl, ParamVal};
use crate::support::{
    convergence_param, diag_clamped, diag_condition, diag_correction, diag_kept, InverseRule,
    Weighting,
};
use crate::BuiltinMimoLaw;
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};
use hertzforge_sdm::linalg::{self, Regularization};
use hertzforge_sdm::{HermitianStack, C64};

const NEEDS: [MimoInput; 1] = [MimoInput::SysidResponse];

/// The settings of [`BuzzMatchedLaw`].
#[derive(Clone, Debug, PartialEq)]
pub struct BuzzConfig {
    /// Pairs with identification coherence below this are left independent.
    pub coherence_floor: f64,
    /// The convergence factor `e`.
    pub convergence: f64,
}

impl Default for BuzzConfig {
    fn default() -> Self {
        BuzzConfig {
            coherence_floor: 0.1,
            convergence: 0.5,
        }
    }
}

impl BuzzConfig {
    /// Set the parameter `key`.
    pub fn set(&mut self, key: &str, v: &ParamVal) -> Result<(), LawError> {
        match key {
            "coherence_floor" => self.coherence_floor = v.number(key)?,
            "convergence" => self.convergence = v.number(key)?,
            other => return Err(LawError(format!("parameter `{other}` is not declared"))),
        }
        Ok(())
    }
}

/// The modified reference: the autospectra of `reference`, the coherence and
/// phase of `identification` for every pair at or above `floor` (independent
/// below it), on the lines of `band` (zero outside), made positive
/// semi-definite with the autospectra kept.
pub fn buzz_reference(
    reference: &HermitianStack,
    identification: &HermitianStack,
    band: (usize, usize),
    floor: f64,
) -> Result<HermitianStack, LawError> {
    let (lines, n) = (reference.lines(), reference.n());
    if identification.lines() != lines || identification.n() != n {
        return Err(LawError(format!(
            "the identification response is {} lines by {}, the reference {lines} by {n}",
            identification.lines(),
            identification.n()
        )));
    }
    let mut g = HermitianStack::zeros(lines, n);
    for line in band.0..band.1 {
        for j in 0..n {
            let gjj = reference.get(line, j, j).re.max(0.0);
            g.set(line, j, j, C64::new(gjj, 0.0));
            for k in (j + 1)..n {
                let (bjj, bkk) = (
                    identification.get(line, j, j).re,
                    identification.get(line, k, k).re,
                );
                let both = bjj > 0.0 && bkk > 0.0;
                if !both {
                    continue;
                }
                let c = identification.get(line, j, k) / (bjj * bkk).sqrt();
                if c.norm_sqr() < floor {
                    continue;
                }
                let gkk = reference.get(line, k, k).re.max(0.0);
                g.set(line, j, k, c * (gjj * gkk).sqrt());
            }
        }
    }
    // Clamp what zeroing a pair made negative, keep every autospectrum. Every
    // negative eigenvalue is clamped however large: the matrix was built
    // here, not requested by a law.
    linalg::psd_repair(&mut g, 1e-12, 1e300)?;
    Ok(g)
}

/// The buzz-test matched law for one run.
pub struct BuzzMatchedLaw {
    setup: MimoSetup,
    cfg: BuzzConfig,
    inner: Option<FactorLoop>,
}

impl BuzzMatchedLaw {
    /// The law for `setup` with `cfg`. The modified reference is formed at the
    /// seed, from the identification response.
    pub fn new(setup: &MimoSetup, cfg: &BuzzConfig) -> Result<Self, LawError> {
        setup.check()?;
        Ok(BuzzMatchedLaw {
            setup: setup.clone(),
            cfg: cfg.clone(),
            inner: None,
        })
    }
}

impl MimoControlLaw for BuzzMatchedLaw {
    fn needs(&self) -> &[MimoInput] {
        &NEEDS
    }

    fn shapes(&self) -> &[MimoShape] {
        &ALL_SHAPES
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let ident = inputs
            .sysid_response
            .ok_or_else(|| LawError("the identification response was not supplied".into()))?;
        let target = buzz_reference(
            &self.setup.reference,
            ident,
            self.setup.band,
            self.cfg.coherence_floor,
        )?;
        let settings = LoopSettings {
            convergence: self.cfg.convergence,
            cap_up_db: 3.5,
            cap_down_db: 6.0,
            rank_tol: 1e-6,
        };
        let rule = InverseRule::Regularized {
            reg: Regularization::Truncated { rcond: 1e-3 },
            weighting: Weighting::None,
        };
        let mut inner = FactorLoop::new(&self.setup, &target, rule, settings)?;
        let out = inner.seed(inputs.frf)?.outputs(true);
        self.inner = Some(inner);
        Ok(out)
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| LawError("iterate was called before seed".into()))?;
        Ok(inner.iterate(inputs.frf, inputs.response)?.outputs(true))
    }
}

impl BuiltinMimoLaw for BuzzMatchedLaw {
    fn spec() -> LawSpec {
        LawSpec {
            name: "buzz-test matched",
            version: "1.0.0",
            author: "Hertzforge",
            summary: "Keeps the specified autospectra and takes each pair's coherence and \
                      phase from the identification response, the structure's own response to \
                      uncorrelated drive, so the cross terms asked for are ones the structure \
                      produces naturally; pairs below the coherence floor are left \
                      independent. Then runs the default law's closed loop on that reference.",
            params: vec![
                ParamDecl {
                    key: "coherence_floor",
                    label: "Coherence floor",
                    help: "Pairs whose identification coherence is below this are left \
                           independent.",
                    kind: ParamKindDecl::Number {
                        unit: "",
                        min: 0.0,
                        max: 1.0,
                        step: 0.01,
                        default: 0.1,
                    },
                },
                convergence_param(),
            ],
            diagnostics: vec![
                diag_condition(),
                diag_kept(),
                diag_correction(),
                diag_clamped(),
            ],
            shapes: ALL_SHAPES.to_vec(),
            needs: NEEDS.to_vec(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        let mut cfg = BuzzConfig::default();
        for (k, v) in params {
            cfg.set(k, v)?;
        }
        Self::new(setup, &cfg)
    }
}
