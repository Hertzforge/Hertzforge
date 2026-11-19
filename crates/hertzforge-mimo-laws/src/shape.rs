//! Law 3: shape-constrained control. The drive is held to the `m` directions
//! the plant responds to most on each line, which trades fidelity in the
//! weak directions for much lower drive.
//!
//! Source: R. Schultz and P. Avitabile, "Shape-Constrained Input Estimation
//! for Efficient Multi-shaker Vibration Testing", Experimental Techniques 44
//! (2020), and their IMAC paper of the same method. With the constraint `C`
//! the `m` leading right singular vectors of `H` on a line: `H_c = H C`,
//! `G_c = H_c^+ G_ref H_c^+^H`, `G_uu = C G_c C^H`. Because `H C = U_m S_m`,
//! the drive's inverse is `V_m S_m^-1 U_m^H`, a truncated-SVD inverse with a
//! fixed kept rank `m` (their own observation). `m` is either fixed, or the
//! number of singular values at or above `tau` times the largest, at most
//! `m_max` (their journal version reports an automatic rule that could not be
//! verified; the ratio rule is the stated alternative).
//!
//! Closed loop: Smallwood's correction ([`crate::factor`]) runs on `H_c` and is
//! mapped back through `C`, so the constraint holds on every loop and no more
//! than `m` independent sources are ever driven.

use crate::factor::{FactorLoop, LoopSettings};
use crate::pinv::ALL_SHAPES;
use crate::spec::{LawSpec, ParamDecl, ParamKindDecl, ParamVal};
use crate::support::{
    convergence_param, diag_clamped, diag_condition, diag_correction, diag_kept, InverseRule,
    ShapeSelection,
};
use crate::BuiltinMimoLaw;
use hertzforge_sdm::law::{
    LawError, MimoControlLaw, MimoInput, MimoLoopInputs, MimoLoopOutputs, MimoSetup, MimoShape,
};

const SELECTIONS: &[&str] = &["ratio", "count"];

/// The settings of [`ShapeConstrainedLaw`].
#[derive(Clone, Debug, PartialEq)]
pub struct ShapeConfig {
    /// `ratio` or `count`.
    pub selection: String,
    /// `m` when the selection is `count`.
    pub shapes: i64,
    /// `tau` when the selection is `ratio`.
    pub ratio: f64,
    /// `m_max` when the selection is `ratio`.
    pub max_shapes: i64,
    /// The convergence factor `e`.
    pub convergence: f64,
}

impl Default for ShapeConfig {
    fn default() -> Self {
        ShapeConfig {
            selection: "ratio".into(),
            shapes: 2,
            ratio: 0.2,
            max_shapes: 1024,
            convergence: 0.5,
        }
    }
}

impl ShapeConfig {
    /// Set the parameter `key`.
    pub fn set(&mut self, key: &str, v: &ParamVal) -> Result<(), LawError> {
        match key {
            "selection" => self.selection = v.choice(key)?.to_string(),
            "shapes" => self.shapes = v.integer(key)?,
            "ratio" => self.ratio = v.number(key)?,
            "max_shapes" => self.max_shapes = v.integer(key)?,
            "convergence" => self.convergence = v.number(key)?,
            other => return Err(LawError(format!("parameter `{other}` is not declared"))),
        }
        Ok(())
    }

    fn selection(&self) -> Result<ShapeSelection, LawError> {
        let whole = |key: &str, v: i64| {
            usize::try_from(v)
                .ok()
                .filter(|n| *n >= 1)
                .ok_or_else(|| LawError(format!("parameter `{key}` must be at least 1")))
        };
        match self.selection.as_str() {
            "count" => Ok(ShapeSelection::Count(whole("shapes", self.shapes)?)),
            "ratio" => Ok(ShapeSelection::Ratio {
                tau: self.ratio,
                max: whole("max_shapes", self.max_shapes)?,
            }),
            other => Err(LawError(format!(
                "selection `{other}` is not one of {}",
                SELECTIONS.join(", ")
            ))),
        }
    }
}

/// The shape-constrained law for one run.
pub struct ShapeConstrainedLaw {
    inner: FactorLoop,
}

impl ShapeConstrainedLaw {
    /// The law for `setup` with `cfg`.
    pub fn new(setup: &MimoSetup, cfg: &ShapeConfig) -> Result<Self, LawError> {
        let settings = LoopSettings {
            convergence: cfg.convergence,
            cap_up_db: 3.5,
            cap_down_db: 6.0,
            rank_tol: 1e-6,
        };
        Ok(ShapeConstrainedLaw {
            inner: FactorLoop::new(
                setup,
                &setup.reference,
                InverseRule::Shape(cfg.selection()?),
                settings,
            )?,
        })
    }
}

impl MimoControlLaw for ShapeConstrainedLaw {
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

impl BuiltinMimoLaw for ShapeConstrainedLaw {
    fn spec() -> LawSpec {
        LawSpec {
            name: "shape-constrained (SVD)",
            version: "1.0.0",
            author: "Hertzforge",
            summary: "Holds the drive on each line to the m directions the plant responds to \
                      most (its leading right singular vectors), so the drive stays where the \
                      plant is strong: much lower drive for some loss of fidelity between \
                      modes. m is fixed, or set per line by a singular-value ratio. Closed loop \
                      by the same factor correction as the default law, run inside the \
                      constraint.",
            params: vec![
                ParamDecl {
                    key: "selection",
                    label: "Directions kept by",
                    help: "Ratio keeps the singular values at or above the ratio times the \
                           largest, at most the maximum; count keeps a fixed number.",
                    kind: ParamKindDecl::Choice {
                        options: SELECTIONS,
                        default: "ratio",
                    },
                },
                ParamDecl {
                    key: "shapes",
                    label: "Directions kept",
                    help: "With selection by count: how many directions to keep on every line \
                           (fewer where the plant's rank is lower).",
                    kind: ParamKindDecl::Integer {
                        unit: "",
                        min: 1,
                        max: 1024,
                        step: 1,
                        default: 2,
                    },
                },
                ParamDecl {
                    key: "ratio",
                    label: "Singular-value ratio",
                    help: "With selection by ratio: keep the singular values at or above this \
                           fraction of the largest.",
                    kind: ParamKindDecl::Number {
                        unit: "",
                        min: 0.1,
                        max: 0.5,
                        step: 0.05,
                        default: 0.2,
                    },
                },
                ParamDecl {
                    key: "max_shapes",
                    label: "Most directions kept",
                    help: "With selection by ratio: never keep more than this many directions.",
                    kind: ParamKindDecl::Integer {
                        unit: "",
                        min: 1,
                        max: 1024,
                        step: 1,
                        default: 1024,
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
            needs: Vec::new(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        let mut cfg = ShapeConfig::default();
        for (k, v) in params {
            cfg.set(k, v)?;
        }
        Self::new(setup, &cfg)
    }
}
