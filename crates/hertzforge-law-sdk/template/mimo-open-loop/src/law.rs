//! The open-loop pseudo-inverse: every loop, the drive is
//!
//! ```text
//! G_uu = H+ G_ref H+^H
//! ```
//!
//! per line, from the plant estimate `H` (controls by drives) and the
//! reference `G_ref`, with `H+` the regularized pseudo-inverse. With more
//! controls than drives this is the least-squares drive, and the response
//! settles at the part of the reference the plant can produce; with fewer, the
//! minimum-norm drive. Sources: D. O. Smallwood, "Multiple Shaker Random
//! Vibration Control: An Update", Proc. IEST 1999 (the drive); R. Schultz, "A
//! Demonstration of Force Estimation and Regularization Methods for
//! Multi-Shaker Testing", IMAC proceedings (truncated and Tikhonov
//! regularization).
//!
//! What a law owns and what the controller owns: the law proposes the
//! full-level drive spectral matrix each loop; the controller owns the level,
//! every limit and every check. It refuses a drive that is not Hermitian, not
//! finite, the wrong size or not zero outside the band, repairs one with small
//! negative eigenvalues, caps how far a drive may move in one loop, and holds
//! every drive under its ceiling.

use hertzforge_law_sdk::mimo::*;
use hertzforge_law_sdk::{ChoiceSpec, NumberSpec, ParamKind, ParamSpec, Params, RandomMimoLaw};

/// The law's state for one run: what it was set up with.
pub struct OpenLoop {
    band: (usize, usize),
    regularization: linalg::Regularization,
}

impl OpenLoop {
    /// The drive for this loop's plant estimate and reference, and the
    /// diagnostics that go with it.
    fn drive(&self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> {
        // H+ per line, and a report of the singular values it kept.
        let (hp, report) =
            linalg::pinv(inputs.frf, self.regularization).map_err(|e| e.to_string())?;
        // H+ G_ref H+^H per line: a congruence, so the drive is positive
        // semi-definite whenever the reference is.
        let mut drive = linalg::congruence(&hp, inputs.reference).map_err(|e| e.to_string())?;
        // The controller refuses a drive that is not zero outside the band.
        linalg::zero_out_of_band(&mut drive, self.band).map_err(|e| e.to_string())?;
        let condition = report
            .cond_kept
            .iter()
            .map(|c| if c.is_finite() { *c } else { 0.0 })
            .collect();
        let kept = report.kept.iter().map(|&k| k as f64).collect();
        Ok(MimoLoopOutputs {
            drive,
            diagnostics: vec![
                DiagnosticValue {
                    key: "condition".into(),
                    data: DiagnosticData::PerLine(condition),
                },
                DiagnosticValue {
                    key: "kept".into(),
                    data: DiagnosticData::PerLine(kept),
                },
            ],
        })
    }
}

impl RandomMimoLaw for OpenLoop {
    fn describe() -> MimoManifest {
        MimoManifestBuilder::new("open-loop pseudo-inverse (example)", "0.1.0")
            .author("Your Name")
            .summary(
                "The regularized pseudo-inverse drive, H+ G_ref H+^H, from the plant estimate \
                 every loop. The worked example of a multiple-output law.",
            )
            .param(ParamSpec {
                key: "regularization".into(),
                label: "Regularization".into(),
                help: "Truncated keeps singular values above the amount times the largest; \
                       Tikhonov damps each by the amount times the largest."
                    .into(),
                kind: ParamKind::Choice(ChoiceSpec {
                    options: vec!["truncated".into(), "tikhonov".into()],
                    default: "truncated".into(),
                }),
            })
            .param(ParamSpec {
                key: "amount".into(),
                label: "Regularization amount".into(),
                help: "Relative to each line's largest singular value. 0.001 trusts the \
                       estimate to a condition number of 1,000."
                    .into(),
                kind: ParamKind::Number(NumberSpec {
                    unit: String::new(),
                    min: 0.0,
                    max: 1.0,
                    step: 1e-4,
                    default: 1e-3,
                }),
            })
            // Every shape: square, more controls than drives (least squares),
            // fewer controls than drives (minimum norm).
            .shapes(&[
                MimoShape::Square,
                MimoShape::MoreControlsThanDrives,
                MimoShape::FewerControlsThanDrives,
            ])
            // No optional inputs: the law reads the reference and the plant
            // estimate only, so the controller sends nothing else.
            .diagnostic(
                "condition",
                "Condition number",
                "",
                "Largest over smallest singular value the inverse used, per line.",
                DiagnosticKind::PerLine,
            )
            .diagnostic(
                "kept",
                "Singular values kept",
                "",
                "How many singular values the inverse used, per line.",
                DiagnosticKind::PerLine,
            )
            .build()
    }

    fn new(setup: MimoSetup, params: Params) -> Result<Self, String> {
        let amount = params.number("amount")?;
        let regularization = match params.choice("regularization")? {
            "truncated" => linalg::Regularization::Truncated { rcond: amount },
            "tikhonov" => linalg::Regularization::Tikhonov { lambda_rel: amount },
            other => {
                return Err(format!(
                    "regularization `{other}` is not one this law offers"
                ))
            }
        };
        Ok(OpenLoop {
            band: setup.band,
            regularization,
        })
    }

    // Called once, after identification, with the identification response as
    // the response. An open-loop law gives the same drive as every loop.
    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> {
        self.drive(inputs)
    }

    // Called every loop at full level. The plant estimate is the one the
    // operator chose: held from identification, or updated each loop.
    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> {
        self.drive(inputs)
    }
}
