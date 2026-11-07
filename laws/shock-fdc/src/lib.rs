//! The built-in classical shock law, `fd_correct`, as a law file. It is the
//! controller's own [`FdCorrect`]: this crate only carries it across the law
//! interface, with the manifest built from the same declaration
//! (`hertzforge_shock_law_math::classical`) as the controller's catalog entry.

use hertzforge_law_sdk::shock::*;
use hertzforge_law_sdk::{
    export_classical_law, DiagnosticSpec, Manifest, NumberSpec, ParamKind, ParamSpec, Params,
    ShockClassicalLaw,
};
use hertzforge_shock_law_math::classical::{
    FdCorrect, FdCorrectParams, Step, DIAGNOSTICS, ID, KNOBS, NAME, SUMMARY, VERSION,
};
use hertzforge_shock_law_math::AUTHOR;

/// `fd_correct` behind the law interface.
pub struct FdCorrectFile {
    math: FdCorrect,
}

fn out(step: Step) -> PulseOutputs {
    outputs(step.drive, &step.report.values())
}

impl ShockClassicalLaw for FdCorrectFile {
    fn describe() -> Manifest {
        let _ = ID;
        Manifest {
            name: NAME.into(),
            version: VERSION.into(),
            author: AUTHOR.into(),
            summary: SUMMARY.into(),
            params: KNOBS
                .iter()
                .map(|k| ParamSpec {
                    key: k.key.into(),
                    label: k.label.into(),
                    help: k.help.into(),
                    kind: ParamKind::Number(NumberSpec {
                        unit: k.unit.into(),
                        min: k.min,
                        max: k.max,
                        step: k.step,
                        default: k.default,
                    }),
                })
                .collect(),
            diagnostics: DIAGNOSTICS
                .iter()
                .map(|d| DiagnosticSpec {
                    key: d.key.into(),
                    label: d.label.into(),
                    unit: d.unit.into(),
                    help: d.help.into(),
                })
                .collect(),
        }
    }

    fn new(setup: ClassicalSetup, params: Params) -> Result<Self, String> {
        let mut p = FdCorrectParams::default();
        for k in KNOBS {
            p.set(k.key, params.number(k.key)?)?;
        }
        let math = FdCorrect::new(
            p,
            setup.record_len as usize,
            &setup.reference,
            &inverse(&setup.plant),
            &setup.plant.trusted,
        )?;
        Ok(FdCorrectFile { math })
    }

    fn seed(&mut self, _inputs: &PulseInputs) -> Result<PulseOutputs, String> {
        self.math.seed().map(out)
    }

    fn iterate(&mut self, inputs: &PulseInputs) -> Result<PulseOutputs, String> {
        self.math.iterate(&inputs.response).map(out)
    }
}

export_classical_law!(FdCorrectFile);
