//! The multiple-output worked example of `docs/control-laws.md`, compiled and
//! run natively through the SDK's trait, so the example in the guide is code
//! that builds and works. (A law file adds one line, `export_mimo_law!(OpenLoop);`,
//! at its crate root.)

// The example as the guide shows it, from here to the end of `impl`.
use hertzforge_law_sdk::mimo::*;
use hertzforge_law_sdk::{NumberSpec, ParamKind, ParamSpec, Params, RandomMimoLaw};

/// The truncated pseudo-inverse drive `H+ G_ref H+^H`, every loop.
pub struct OpenLoop {
    band: (usize, usize),
    rcond: f64,
}

impl RandomMimoLaw for OpenLoop {
    fn describe() -> MimoManifest {
        MimoManifestBuilder::new("open-loop example", "0.1.0")
            .summary("The truncated pseudo-inverse drive, every loop.")
            .param(ParamSpec {
                key: "rcond".into(),
                label: "Singular values kept above".into(),
                help: "Keep singular values above this fraction of the largest.".into(),
                kind: ParamKind::Number(NumberSpec {
                    unit: String::new(),
                    min: 0.0,
                    max: 1.0,
                    step: 1e-4,
                    default: 1e-3,
                }),
            })
            .shapes(&[
                MimoShape::Square,
                MimoShape::MoreControlsThanDrives,
                MimoShape::FewerControlsThanDrives,
            ])
            .diagnostic(
                "kept",
                "Singular values kept",
                "",
                "Per line.",
                DiagnosticKind::PerLine,
            )
            .build()
    }

    fn new(setup: MimoSetup, params: Params) -> Result<Self, String> {
        Ok(OpenLoop {
            band: setup.band,
            rcond: params.number("rcond")?,
        })
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> {
        self.iterate(inputs)
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> {
        let reg = linalg::Regularization::Truncated { rcond: self.rcond };
        let (hp, report) = linalg::pinv(inputs.frf, reg).map_err(|e| e.to_string())?;
        let mut drive = linalg::congruence(&hp, inputs.reference).map_err(|e| e.to_string())?;
        linalg::zero_out_of_band(&mut drive, self.band).map_err(|e| e.to_string())?;
        let kept = report.kept.iter().map(|&k| k as f64).collect();
        Ok(MimoLoopOutputs {
            drive,
            diagnostics: vec![DiagnosticValue {
                key: "kept".into(),
                data: DiagnosticData::PerLine(kept),
            }],
        })
    }
}

// -------------------------------------------------------------------------

use hertzforge_law_sdk::{Param, ParamValue};

#[test]
fn the_worked_example_returns_the_pseudo_inverse_drive() {
    let (lines, band) = (8, (2, 6));
    let mut reference = HermitianStack::zeros(lines, 3);
    let mut frf = MatrixStack::zeros(lines, 3, 2);
    for line in 0..lines {
        for i in 0..3 {
            reference.set(line, i, i, C64::new(1e-2, 0.0));
        }
        frf.set(line, 0, 0, C64::new(2.0, 0.0));
        frf.set(line, 1, 1, C64::new(1.0, 0.0));
        frf.set(line, 2, 0, C64::new(0.5, 0.5));
    }
    let setup = MimoSetup {
        controls: 3,
        drives: 2,
        lines,
        df_hz: 1.0,
        band,
        reference: reference.clone(),
        tolerances: Tolerances::none(lines, 3),
    };
    let m = OpenLoop::describe();
    assert_eq!(m.law.params.len(), 1);
    assert_eq!(m.shapes.len(), 3);
    let params = Params(vec![Param {
        key: "rcond".into(),
        value: ParamValue::Number(1e-3),
    }]);
    let mut law = OpenLoop::new(setup.clone(), params).expect("builds");
    let coherence = vec![1.0; lines * 3];
    let inputs = MimoLoopInputs {
        reference: &reference,
        tolerances: &setup.tolerances,
        response: &reference,
        response_dof: 64.0,
        frf: &frf,
        multiple_coherence: &coherence,
        df_hz: 1.0,
        band,
        loop_index: 0,
        last_drive: None,
        response_drive_csd: None,
        sysid_response: None,
        sysid_drive: None,
        sysid_noise: None,
    };
    let out = law.seed(&inputs).expect("seeds");
    // Out of band, nothing; in band, a drive the plant turns into the
    // least-squares response.
    assert_eq!(out.drive.trace(0), 0.0);
    assert!(out.drive.trace(3) > 0.0);
    let (hp, _) = linalg::pinv(&frf, linalg::Regularization::Truncated { rcond: 1e-3 }).unwrap();
    let want = linalg::congruence(&hp, &reference).unwrap();
    assert_eq!(out.drive.line(3), want.line(3));
}
