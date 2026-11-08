//! The built-in SRS shock law, `srs_ratio`, as a law file. It is the
//! controller's own [`SrsRatio`]: this crate only carries it across the law
//! interface, with the manifest built from the same declaration
//! (`hertzforge_shock_law_math::srs`) as the controller's catalog entry.

use hertzforge_law_sdk::srs::*;
use hertzforge_law_sdk::{
    export_srs_law, DiagnosticSpec, Manifest, NumberSpec, ParamKind, ParamSpec, Params, ShockSrsLaw,
};
use hertzforge_shock_law_math::srs::{
    SrsRatio, SrsRatioParams, Step, DIAGNOSTICS, ID, KNOBS, NAME, SUMMARY, VERSION,
};
use hertzforge_shock_law_math::AUTHOR;

/// `srs_ratio` behind the law interface.
pub struct SrsRatioFile {
    math: SrsRatio,
}

fn out(step: Step) -> SrsOutputs {
    srs_outputs(step.gains, &step.report.values())
}

impl ShockSrsLaw for SrsRatioFile {
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

    fn new(setup: SrsSetup, params: Params) -> Result<Self, String> {
        let mut p = SrsRatioParams::default();
        for k in KNOBS {
            p.set(k.key, params.number(k.key)?)?;
        }
        let math = SrsRatio::new(
            p,
            &setup.band_hz,
            &setup.component_hz,
            setup.component_gain.as_deref(),
        )?;
        Ok(SrsRatioFile { math })
    }

    fn seed(&mut self, _inputs: &SrsInputs) -> Result<SrsOutputs, String> {
        self.math.seed().map(out)
    }

    fn iterate(&mut self, inputs: &SrsInputs) -> Result<SrsOutputs, String> {
        self.math.iterate(&inputs.target, &inputs.measured).map(out)
    }
}

export_srs_law!(SrsRatioFile);
