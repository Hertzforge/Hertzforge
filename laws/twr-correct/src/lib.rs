//! The built-in TWR law, `twr_correct`, as a law file. It is the controller's
//! own [`TwrCorrect`]: this crate only carries it across the law interface,
//! with the manifest built from the same declaration
//! (`hertzforge_shock_law_math::twr`) as the controller's catalog entry.

use hertzforge_law_sdk::twr::*;
use hertzforge_law_sdk::{
    export_twr_law, DiagnosticSpec, Manifest, NumberSpec, ParamKind, ParamSpec, Params,
    TwrSingleLaw,
};
use hertzforge_shock_law_math::classical::Step;
use hertzforge_shock_law_math::twr::{
    TwrCorrect, TwrCorrectParams, DIAGNOSTICS, ID, KNOBS, NAME, SUMMARY, VERSION,
};
use hertzforge_shock_law_math::AUTHOR;

/// `twr_correct` behind the law interface.
pub struct TwrCorrectFile {
    math: TwrCorrect,
}

fn out(step: Step) -> PlayOutputs {
    outputs(step.drive, &step.report.values())
}

impl TwrSingleLaw for TwrCorrectFile {
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

    fn new(setup: TwrSetup, params: Params) -> Result<Self, String> {
        let mut p = TwrCorrectParams::default();
        for k in KNOBS {
            p.set(k.key, params.number(k.key)?)?;
        }
        let math = TwrCorrect::new(
            p,
            setup.record_len as usize,
            &setup.reference,
            &inverse(&setup.plant),
            &setup.plant.trusted,
        )?;
        Ok(TwrCorrectFile { math })
    }

    fn seed(&mut self, _inputs: &PlayInputs) -> Result<PlayOutputs, String> {
        self.math.seed().map(out)
    }

    fn iterate(&mut self, inputs: &PlayInputs) -> Result<PlayOutputs, String> {
        self.math
            .iterate(
                &inputs.response,
                inputs.limited.then_some(inputs.drive.as_slice()),
            )
            .map(out)
    }
}

export_twr_law!(TwrCorrectFile);
