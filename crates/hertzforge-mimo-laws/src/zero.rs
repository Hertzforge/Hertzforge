//! The zero law: always returns the zero drive. It exists to prove the whole
//! path a multiple-output law takes (the SDK's export, the law-file build, the
//! host's binding and checks, the catalog and the adapter) before any real law
//! runs on it, and reports one diagnostic of every layout so each is carried
//! end to end. It is not a control law anyone should run.

use crate::spec::{DiagDecl, DiagKind, LawSpec, ParamDecl, ParamKindDecl, ParamVal};
use crate::BuiltinMimoLaw;
use hertzforge_sdm::law::{
    DiagnosticData, DiagnosticValue, LawError, MimoControlLaw, MimoInput, MimoLoopInputs,
    MimoLoopOutputs, MimoSetup, MimoShape,
};
use hertzforge_sdm::HermitianStack;

const SHAPES: [MimoShape; 3] = [
    MimoShape::Square,
    MimoShape::MoreControlsThanDrives,
    MimoShape::FewerControlsThanDrives,
];
const NEEDS: [MimoInput; 1] = [MimoInput::SysidResponse];

/// The zero law's state for one run.
#[derive(Debug, Clone)]
pub struct ZeroLaw {
    lines: usize,
    drives: usize,
    band: (usize, usize),
    gain: f64,
}

impl ZeroLaw {
    fn outputs(&self, i: &MimoLoopInputs<'_>) -> MimoLoopOutputs {
        let (lo, hi) = self.band;
        let nc = i.response.n();
        let mut response_psd = vec![0.0; self.lines * nc];
        for line in 0..self.lines {
            for (c, v) in i.response.diagonal(line).into_iter().enumerate() {
                response_psd[line * nc + c] = self.gain * v;
            }
        }
        let sysid_trace = i
            .sysid_response
            .map(|s| (lo..hi).map(|l| s.trace(l)).sum::<f64>())
            .unwrap_or(0.0);
        let value = |key: &str, data| DiagnosticValue {
            key: key.into(),
            data,
        };
        MimoLoopOutputs {
            drive: HermitianStack::zeros(self.lines, self.drives),
            diagnostics: vec![
                value("loop", DiagnosticData::Scalar(i.loop_index as f64)),
                value(
                    "in_band",
                    DiagnosticData::PerLine(
                        (0..self.lines)
                            .map(|l| if (lo..hi).contains(&l) { 1.0 } else { 0.0 })
                            .collect(),
                    ),
                ),
                value("response_psd", DiagnosticData::PerControlLine(response_psd)),
                value(
                    "drive_psd",
                    DiagnosticData::PerDriveLine(vec![0.0; self.lines * self.drives]),
                ),
                value("sysid_trace", DiagnosticData::Scalar(sysid_trace)),
            ],
        }
    }
}

impl MimoControlLaw for ZeroLaw {
    fn needs(&self) -> &[MimoInput] {
        &NEEDS
    }

    fn shapes(&self) -> &[MimoShape] {
        &SHAPES
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        Ok(self.outputs(inputs))
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, LawError> {
        Ok(self.outputs(inputs))
    }
}

impl BuiltinMimoLaw for ZeroLaw {
    fn spec() -> LawSpec {
        LawSpec {
            name: "zero drive",
            version: "0.1.0",
            author: "Hertzforge",
            summary: "Returns the zero drive on every loop and reports one diagnostic of each \
                      layout. It proves the path a multiple-output law takes; it controls \
                      nothing.",
            params: vec![ParamDecl {
                key: "gain",
                label: "Diagnostic gain",
                help: "Scales the reported response PSD. The drive is zero whatever it is.",
                kind: ParamKindDecl::Number {
                    unit: "",
                    min: 0.0,
                    max: 10.0,
                    step: 0.1,
                    default: 1.0,
                },
            }],
            diagnostics: vec![
                DiagDecl {
                    key: "loop",
                    label: "Loop",
                    unit: "",
                    help: "The loop index the law was given.",
                    kind: DiagKind::Scalar,
                },
                DiagDecl {
                    key: "in_band",
                    label: "In band",
                    unit: "",
                    help: "1 on lines inside the band, 0 outside.",
                    kind: DiagKind::PerLine,
                },
                DiagDecl {
                    key: "response_psd",
                    label: "Response PSD",
                    unit: "EU^2/Hz",
                    help: "The measured response's diagonal, times the gain.",
                    kind: DiagKind::PerControlLine,
                },
                DiagDecl {
                    key: "drive_psd",
                    label: "Drive PSD",
                    unit: "V^2/Hz",
                    help: "The drive's diagonal: zero.",
                    kind: DiagKind::PerDriveLine,
                },
                DiagDecl {
                    key: "sysid_trace",
                    label: "Identification trace",
                    unit: "EU^2/Hz",
                    help: "The identification response's trace, summed over the band.",
                    kind: DiagKind::Scalar,
                },
            ],
            shapes: SHAPES.to_vec(),
            needs: NEEDS.to_vec(),
        }
    }

    fn build(setup: &MimoSetup, params: &[(String, ParamVal)]) -> Result<Self, LawError> {
        setup.check()?;
        let mut gain = 1.0;
        for (key, v) in params {
            match key.as_str() {
                "gain" => gain = v.number(key)?,
                other => return Err(LawError(format!("parameter `{other}` is not declared"))),
            }
        }
        Ok(ZeroLaw {
            lines: setup.lines,
            drives: setup.drives,
            band: setup.band,
            gain,
        })
    }
}
