//! Law 5: the minimum-drive target. The reference keeps its specified
//! autospectra, as far as the trade-off allows, and takes its cross terms from
//! the drive that needs the least power to produce them: on each line the
//! drive minimizing the weighted autospectrum residuals plus a trade-off times
//! the drive power, over the positive semi-definite cone, and the plant's
//! predicted response to it as the target. The default law's closed loop then
//! holds that target.
//!
//! Sources: K. Coletti, R. B. Davis and R. Schultz, "Pareto-optimal target
//! definition for multi-axis random vibration testing", journal preprint,
//! 2026 (OSTI 3023856), whose problem this is, with their
//! automatic choice of the trade-off at the knee of drive against residual;
//! D. O. Smallwood, "Multiple-Input Multiple-Output (MIMO) Linear Systems
//! Extreme Inputs/Outputs", Shock and Vibration 14 (2007), and "Minimum Input
//! Trace for Multiple Input Multiple Output Linear Systems", Journal of the
//! IEST 56 (2013), the minimum input trace among the drives that reproduce the
//! autospectra exactly, which is the small trade-off end of the same problem.
//! The solve is [`crate::pareto`].
//!
//! What the law gives up: with the trade-off above its exact-control end, the
//! target's autospectra sit below the specification where the plant pays most
//! for them, by a ratio the `target_ratio` diagnostic reports per control per
//! line, and the core judges the run against the specification's alarm and
//! abort lines as it does for every law.

use crate::factor::{FactorLoop, LoopSettings};
use crate::pareto::{self, ResidualWeight, Target, Tradeoff};
use crate::pinv::ALL_SHAPES;
use crate::spec::{DiagDecl, DiagKind, LawSpec, ParamDecl, ParamKindDecl, ParamVal};
use crate::support::{convergence_param, diag_clamped, diag_condition, diag_correction, diag_kept};
use crate::BuiltinMimoLaw;
use hertzforge_sdm::law::{
    DiagnosticData, DiagnosticValue, LawError, MimoControlLaw, MimoInput, MimoLoopInputs,
    MimoLoopOutputs, MimoSetup, MimoShape,
};

const TRADEOFFS: &[&str] = &["auto", "fixed"];
const RESIDUALS: &[&str] = &["relative", "absolute"];

/// The settings of [`MinimumDriveLaw`].
#[derive(Clone, Debug, PartialEq)]
pub struct MinimumDriveConfig {
    /// `auto` or `fixed`.
    pub tradeoff: String,
    /// The relative trade-off when `fixed`.
    pub tradeoff_value: f64,
    /// `relative` or `absolute`.
    pub residual: String,
    /// The convergence factor `e`.
    pub convergence: f64,
}

impl Default for MinimumDriveConfig {
    fn default() -> Self {
        MinimumDriveConfig {
            tradeoff: "auto".into(),
            tradeoff_value: 1.0,
            residual: "relative".into(),
            convergence: 0.5,
        }
    }
}

impl MinimumDriveConfig {
    /// Set the parameter `key`.
    pub fn set(&mut self, key: &str, v: &ParamVal) -> Result<(), LawError> {
        match key {
            "tradeoff" => self.tradeoff = v.choice(key)?.to_string(),
            "tradeoff_value" => self.tradeoff_value = v.number(key)?,
            "residual" => self.residual = v.choice(key)?.to_string(),
            "convergence" => self.convergence = v.number(key)?,
            other => return Err(LawError(format!("parameter `{other}` is not declared"))),
        }
        Ok(())
    }

    fn tradeoff(&self) -> Result<Tradeoff, LawError> {
        match self.tradeoff.as_str() {
            "auto" => Ok(Tradeoff::Auto),
            "fixed" => {
                if self.tradeoff_value <= 0.0 || !self.tradeoff_value.is_finite() {
                    return Err(LawError(format!(
                        "parameter `tradeoff_value` must be positive, got {}",
                        self.tradeoff_value
                    )));
                }
                Ok(Tradeoff::Fixed(self.tradeoff_value))
            }
            other => Err(LawError(format!(
                "tradeoff `{other}` is not one of {}",
                TRADEOFFS.join(", ")
            ))),
        }
    }

    fn residual(&self) -> Result<ResidualWeight, LawError> {
        match self.residual.as_str() {
            "relative" => Ok(ResidualWeight::Relative),
            "absolute" => Ok(ResidualWeight::Absolute),
            other => Err(LawError(format!(
                "residual `{other}` is not one of {}",
                RESIDUALS.join(", ")
            ))),
        }
    }
}

/// The minimum-drive law for one run. The target is formed at the seed, from
/// the plant estimate the core hands the law.
pub struct MinimumDriveLaw {
    setup: MimoSetup,
    cfg: MinimumDriveConfig,
    inner: Option<FactorLoop>,
    /// The target's diagnostics, reported on every loop.
    target: Option<Target>,
}

impl MinimumDriveLaw {
    /// The law for `setup` with `cfg`.
    pub fn new(setup: &MimoSetup, cfg: &MinimumDriveConfig) -> Result<Self, LawError> {
        setup.check()?;
        cfg.tradeoff()?;
        cfg.residual()?;
        Ok(MinimumDriveLaw {
            setup: setup.clone(),
            cfg: cfg.clone(),
            inner: None,
            target: None,
        })
    }

    fn with_target_diagnostics(&self, mut out: MimoLoopOutputs) -> MimoLoopOutputs {
        if let Some(t) = &self.target {
            let value = |key: &str, data| DiagnosticValue {
                key: key.into(),
                data,
            };
            out.diagnostics
                .push(value("tradeoff", DiagnosticData::Scalar(t.tradeoff)));
            out.diagnostics.push(value(
                "drive_power",
                DiagnosticData::PerLine(t.drive_power.clone()),
            ));
            out.diagnostics.push(value(
                "target_ratio",
                DiagnosticData::PerControlLine(t.target_ratio.clone()),
            ));
        }
        out
    }
}

impl MimoControlLaw for MinimumDriveLaw {
    fn needs(&self) -> &[MimoInput] {
        &[]
    }

    fn shapes(&self) -> &[MimoShape] {
        &ALL_SHAPES
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let target = pareto::target(
            inputs.frf,
            &self.setup.reference,
            &self.setup.tolerances,
            self.setup.band,
            self.cfg.residual()?,
            self.cfg.tradeoff()?,
        )?;
        let settings = LoopSettings {
            convergence: self.cfg.convergence,
            cap_up_db: 3.5,
            cap_down_db: 6.0,
            rank_tol: 1e-6,
        };
        let mut inner =
            FactorLoop::new(&self.setup, &target.target, pareto::loop_rule(), settings)?;
        let out = inner.seed(inputs.frf)?.outputs(true);
        self.inner = Some(inner);
        self.target = Some(target);
        Ok(self.with_target_diagnostics(out))
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        let inner = self
            .inner
            .as_mut()
            .ok_or_else(|| LawError("iterate was called before seed".into()))?;
        let out = inner.iterate(inputs.frf, inputs.response)?.outputs(true);
        Ok(self.with_target_diagnostics(out))
    }
}

impl BuiltinMimoLaw for MinimumDriveLaw {
    fn spec() -> LawSpec {
        LawSpec {
            name: "minimum drive",
            version: "1.0.0",
            author: "Hertzforge",
            summary: "Takes the cross terms of the reference from the drive that needs the least \
                      power to produce its autospectra: on each line the drive minimizing the \
                      autospectrum residuals plus a trade-off times the drive power, over the \
                      positive semi-definite cone (the Pareto-optimal target of Coletti, Davis \
                      and Schultz; Smallwood's minimum input trace at the exact-control end), \
                      and the plant's predicted response to it as the target. The trade-off is \
                      picked at the knee of drive against residual among the trade-offs whose \
                      target stays inside the alarm lines, or set. Then the default law's \
                      closed loop on that target.",
            params: vec![
                ParamDecl {
                    key: "tradeoff",
                    label: "Trade-off",
                    help: "Auto picks the trade-off between drive power and autospectrum \
                           residual at the knee of the curve over the band, among the \
                           trade-offs whose target stays inside the alarm lines; fixed uses \
                           the value below.",
                    kind: ParamKindDecl::Choice {
                        options: TRADEOFFS,
                        default: "auto",
                    },
                },
                ParamDecl {
                    key: "tradeoff_value",
                    label: "Trade-off value",
                    help: "With a fixed trade-off: the price of a unit of drive power, relative \
                           to the least power that puts each control's autospectrum on it \
                           alone. Near 1e-6 the autospectra are reproduced where the plant \
                           can, with the least drive that does so; larger values buy lower \
                           drive with larger residuals.",
                    kind: ParamKindDecl::Number {
                        unit: "",
                        min: 1e-6,
                        max: 1e6,
                        step: 0.1,
                        default: 1.0,
                    },
                },
                ParamDecl {
                    key: "residual",
                    label: "Residual weighting",
                    help: "Relative judges each control by the fraction of its autospectrum it \
                           misses by; absolute by the missed level itself, so loud controls \
                           count for more.",
                    kind: ParamKindDecl::Choice {
                        options: RESIDUALS,
                        default: "relative",
                    },
                },
                convergence_param(),
            ],
            diagnostics: vec![
                diag_condition(),
                diag_kept(),
                diag_correction(),
                diag_clamped(),
                DiagDecl {
                    key: "tradeoff",
                    label: "Trade-off used",
                    unit: "",
                    help: "The relative trade-off the target was formed with.",
                    kind: DiagKind::Scalar,
                },
                DiagDecl {
                    key: "drive_power",
                    label: "Target drive power",
                    unit: "V^2/Hz",
                    help: "The trace of the drive the target was derived from, per line.",
                    kind: DiagKind::PerLine,
                },
                DiagDecl {
                    key: "target_ratio",
                    label: "Target over specification",
                    unit: "",
                    help: "The target's autospectrum over the specified one, per control per \
                           line: 1 where the target is as specified, below 1 by what the \
                           trade-off gave up.",
                    kind: DiagKind::PerControlLine,
                },
            ],
            shapes: ALL_SHAPES.to_vec(),
            needs: Vec::new(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        let mut cfg = MinimumDriveConfig::default();
        for (k, v) in params {
            cfg.set(k, v)?;
        }
        Self::new(setup, &cfg)
    }
}
