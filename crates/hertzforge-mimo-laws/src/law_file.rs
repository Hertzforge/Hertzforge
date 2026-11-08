//! A built-in law as a law file: [`LawFile`] implements the SDK's
//! [`RandomMimoLaw`] over any [`BuiltinMimoLaw`], so a law file crate is one
//! line, `export_mimo_law!(LawFile<TheLaw>)`, and the law file runs exactly the
//! code the controller runs natively. Its manifest is built from the law's
//! [`LawSpec`], the same declaration the controller's catalog entry is built
//! from.

use crate::spec::{DiagKind, LawSpec, ParamKindDecl, ParamVal};
use crate::BuiltinMimoLaw;
use hertzforge_law_sdk::mimo::{
    DiagnosticKind, MimoLoopInputs, MimoLoopOutputs, MimoManifest, MimoManifestBuilder, MimoSetup,
};
use hertzforge_law_sdk::{
    ChoiceSpec, IntegerSpec, NumberSpec, ParamKind, ParamSpec, ParamValue, Params, RandomMimoLaw,
};

/// A built-in law, exported as a law file.
pub struct LawFile<L: BuiltinMimoLaw>(pub L);

/// The SDK manifest of a law declared by `spec`.
pub fn sdk_manifest(spec: &LawSpec) -> MimoManifest {
    let mut b = MimoManifestBuilder::new(spec.name, spec.version)
        .author(spec.author)
        .summary(spec.summary)
        .shapes(&spec.shapes)
        .needs(&spec.needs);
    for p in &spec.params {
        b = b.param(ParamSpec {
            key: p.key.into(),
            label: p.label.into(),
            help: p.help.into(),
            kind: match &p.kind {
                ParamKindDecl::Number {
                    unit,
                    min,
                    max,
                    step,
                    default,
                } => ParamKind::Number(NumberSpec {
                    unit: (*unit).into(),
                    min: *min,
                    max: *max,
                    step: *step,
                    default: *default,
                }),
                ParamKindDecl::Integer {
                    unit,
                    min,
                    max,
                    step,
                    default,
                } => ParamKind::Integer(IntegerSpec {
                    unit: (*unit).into(),
                    min: *min,
                    max: *max,
                    step: *step,
                    default: *default,
                }),
                ParamKindDecl::Toggle(d) => ParamKind::Toggle(*d),
                ParamKindDecl::Choice { options, default } => ParamKind::Choice(ChoiceSpec {
                    options: options.iter().map(|o| (*o).to_string()).collect(),
                    default: (*default).into(),
                }),
            },
        });
    }
    for d in &spec.diagnostics {
        b = b.diagnostic(
            d.key,
            d.label,
            d.unit,
            d.help,
            match d.kind {
                DiagKind::Scalar => DiagnosticKind::Scalar,
                DiagKind::PerLine => DiagnosticKind::PerLine,
                DiagKind::PerControlLine => DiagnosticKind::PerControlLine,
                DiagKind::PerDriveLine => DiagnosticKind::PerDriveLine,
            },
        );
    }
    b.build()
}

/// The values the host passed, as a built-in law reads them.
fn values(params: Params) -> Result<Vec<(String, ParamVal)>, String> {
    params
        .0
        .into_iter()
        .map(|p| {
            let v = match p.value {
                ParamValue::Number(v) => ParamVal::Number(v),
                ParamValue::Integer(v) => ParamVal::Integer(v),
                ParamValue::Toggle(v) => ParamVal::Toggle(v),
                ParamValue::Choice(v) => ParamVal::Choice(v),
                ParamValue::Curve(_) | ParamValue::PerChannel(_) => {
                    return Err(format!(
                        "parameter `{}` is of a kind no built-in law declares",
                        p.key
                    ))
                }
            };
            Ok((p.key, v))
        })
        .collect()
}

impl<L: BuiltinMimoLaw> RandomMimoLaw for LawFile<L> {
    fn describe() -> MimoManifest {
        sdk_manifest(&L::spec())
    }

    fn new(setup: MimoSetup, params: Params) -> Result<Self, String> {
        let params = values(params)?;
        L::build(&setup, &params).map(LawFile).map_err(|e| e.0)
    }

    fn seed(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> {
        self.0.seed(inputs).map_err(|e| e.0)
    }

    fn iterate(&mut self, inputs: &MimoLoopInputs<'_>) -> Result<MimoLoopOutputs, String> {
        self.0.iterate(inputs).map_err(|e| e.0)
    }
}
