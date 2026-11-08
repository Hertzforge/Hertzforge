//! Multiple-output Random laws in the catalog: the built-in laws' manifests,
//! the factory a MIMO run builds its law with, and the resolution of a choice
//! of MIMO law.
//!
//! A built-in law ([`BuiltinMimo`]) runs natively; an imported `random-mimo`
//! law file runs through the law host's [`WasmMimoLaw`]. Either way the run
//! holds a `Box<dyn MimoControlLaw + Send>`, so it guards both alike.

use crate::catalog::{LawCatalog, LawWorld};
use crate::choice::{ChoiceError, LawChoice};
use hertzforge_law_host::validate::resolve_params;
use hertzforge_law_host::{
    AnyLaw, ChoiceSpec, DiagnosticKind, DiagnosticLayout, DiagnosticSpec, IntegerSpec, LawError,
    LawIdentity, LoadedMimoLaw, Manifest, MimoManifest, NumberSpec, Param, ParamKind, ParamSpec,
    ParamValue, WasmMimoLaw,
};
use hertzforge_mimo_laws::{BuiltinMimo, DiagKind, LawSpec, ParamKindDecl, ParamVal};
use hertzforge_sdm::law::{MimoControlLaw, MimoSetup};
use std::sync::Arc;
use std::time::Duration;

pub use hertzforge_mimo_laws::DEFAULT_MIMO_LAW_ID;

/// The manifest of a built-in multiple-output law, built from the same
/// declaration its law file's manifest is (a test holds the two equal).
pub fn builtin_mimo_manifest(b: BuiltinMimo) -> MimoManifest {
    manifest_of(&b.spec())
}

fn manifest_of(spec: &LawSpec) -> MimoManifest {
    let params = spec
        .params
        .iter()
        .map(|p| ParamSpec {
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
        })
        .collect();
    MimoManifest {
        law: Manifest {
            name: spec.name.into(),
            version: spec.version.into(),
            author: spec.author.into(),
            summary: spec.summary.into(),
            params,
            diagnostics: spec
                .diagnostics
                .iter()
                .map(|d| DiagnosticSpec {
                    key: d.key.into(),
                    label: d.label.into(),
                    unit: d.unit.into(),
                    help: d.help.into(),
                })
                .collect(),
        },
        shapes: spec.shapes.clone(),
        needs: spec.needs.clone(),
        diagnostic_layouts: spec
            .diagnostics
            .iter()
            .map(|d| DiagnosticLayout {
                key: d.key.into(),
                kind: match d.kind {
                    DiagKind::Scalar => DiagnosticKind::Scalar,
                    DiagKind::PerLine => DiagnosticKind::PerLine,
                    DiagKind::PerControlLine => DiagnosticKind::PerControlLine,
                    DiagKind::PerDriveLine => DiagnosticKind::PerDriveLine,
                },
            })
            .collect(),
    }
}

/// Builds a multiple-output law for each run, with the parameter values it was
/// chosen with (complete and checked).
pub trait MimoLawFactory: Send + Sync {
    /// Which law this builds (an empty hash for a built-in law).
    fn identity(&self) -> LawIdentity;

    /// The complete parameter values, in manifest order (for the run record).
    fn params(&self) -> &[Param];

    /// The law built for a run of `setup`. A law file gets `call_budget` as
    /// the deadline of its constructor and of every call after it; a built-in
    /// law runs natively, and the run measures its calls against the same
    /// budget. Fails when the law refuses the setup (a shape it does not
    /// support, dimensions it cannot run).
    fn build(
        &self,
        setup: &MimoSetup,
        call_budget: Duration,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError>;
}

/// A built-in law and its parameter values.
#[derive(Debug, Clone)]
pub struct BuiltinMimoFactory {
    law: BuiltinMimo,
    params: Vec<Param>,
}

impl BuiltinMimoFactory {
    /// A factory for `law` with `params` (complete and checked).
    pub fn new(law: BuiltinMimo, params: Vec<Param>) -> Self {
        BuiltinMimoFactory { law, params }
    }

    /// The law this factory builds.
    pub fn law(&self) -> BuiltinMimo {
        self.law
    }
}

fn value(p: &Param) -> Result<ParamVal, ChoiceError> {
    Ok(match &p.value {
        ParamValue::Number(v) => ParamVal::Number(*v),
        ParamValue::Integer(v) => ParamVal::Integer(*v),
        ParamValue::Toggle(v) => ParamVal::Toggle(*v),
        ParamValue::Choice(v) => ParamVal::Choice(v.clone()),
        ParamValue::Curve(_) | ParamValue::PerChannel(_) => {
            return Err(ChoiceError::BadParam {
                key: p.key.clone(),
                reason: "no built-in law declares a parameter of this kind".into(),
            })
        }
    })
}

impl MimoLawFactory for BuiltinMimoFactory {
    fn identity(&self) -> LawIdentity {
        let m = builtin_mimo_manifest(self.law).law;
        LawIdentity {
            name: m.name,
            version: m.version,
            author: m.author,
            sha256: String::new(),
        }
    }

    fn params(&self) -> &[Param] {
        &self.params
    }

    fn build(
        &self,
        setup: &MimoSetup,
        _call_budget: Duration,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        let vals = self
            .params
            .iter()
            .map(|p| Ok((p.key.clone(), value(p)?)))
            .collect::<Result<Vec<_>, ChoiceError>>()?;
        self.law
            .build(setup, &vals)
            .map_err(|e| ChoiceError::BadConfig(e.0))
    }
}

/// A multiple-output law file and its parameter values.
#[derive(Clone)]
pub struct WasmMimoLawFactory {
    law: LoadedMimoLaw,
    params: Vec<Param>,
}

impl WasmMimoLawFactory {
    /// A factory for `law` with `params` (complete and checked).
    pub fn new(law: LoadedMimoLaw, params: Vec<Param>) -> Self {
        WasmMimoLawFactory { law, params }
    }

    /// The law this factory builds.
    pub fn law(&self) -> &LoadedMimoLaw {
        &self.law
    }
}

impl MimoLawFactory for WasmMimoLawFactory {
    fn identity(&self) -> LawIdentity {
        self.law.identity().clone()
    }

    fn params(&self) -> &[Param] {
        &self.params
    }

    fn build(
        &self,
        setup: &MimoSetup,
        call_budget: Duration,
    ) -> Result<Box<dyn MimoControlLaw + Send>, ChoiceError> {
        if call_budget.is_zero() {
            return Err(ChoiceError::BadConfig(
                "a law call budget of zero is not a usable time".into(),
            ));
        }
        WasmMimoLaw::build(&self.law, setup, &self.params, call_budget)
            .map(|w| Box::new(w) as Box<dyn MimoControlLaw + Send>)
            .map_err(|e| match e {
                LawError::ParamInvalid { key, reason } => ChoiceError::BadParam { key, reason },
                other => ChoiceError::BadConfig(other.to_string()),
            })
    }
}

impl LawCatalog {
    /// Resolve `choice` for a multiple-output run with `controls` control
    /// channels: find the law, check the parameter values against its
    /// manifest (completing them with defaults; a per-channel parameter has
    /// one value per control), and return the factory the run builds the law
    /// with. A single-output law is refused.
    pub fn resolve_mimo(
        &self,
        choice: &LawChoice,
        controls: usize,
    ) -> Result<Arc<dyn MimoLawFactory>, ChoiceError> {
        let entry = self
            .entry(&choice.id)
            .ok_or_else(|| ChoiceError::UnknownLaw(choice.id.clone()))?;
        if entry.world != LawWorld::Mimo {
            return Err(ChoiceError::WrongTestType {
                id: choice.id.clone(),
                law_is_for: "single-output Random",
            });
        }
        let params =
            resolve_params(&entry.manifest, &choice.params, controls).map_err(|e| match e {
                LawError::ParamInvalid { key, reason } => ChoiceError::BadParam { key, reason },
                other => ChoiceError::BadConfig(other.to_string()),
            })?;
        if let Some(b) = BuiltinMimo::from_id(&choice.id) {
            return Ok(Arc::new(BuiltinMimoFactory::new(b, params)));
        }
        match self.imported_law(&choice.id).map(|i| &i.law) {
            Some(AnyLaw::Mimo(law)) => Ok(Arc::new(WasmMimoLawFactory::new(law.clone(), params))),
            _ => Err(ChoiceError::UnknownLaw(choice.id.clone())),
        }
    }
}

/// The default multiple-output law with its default parameters.
pub fn default_mimo_choice() -> LawChoice {
    LawChoice::new(DEFAULT_MIMO_LAW_ID, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_law_host::mimo::validate_mimo_manifest;

    #[test]
    fn every_built_in_mimo_manifest_passes_the_rules_an_imported_one_must() {
        for b in BuiltinMimo::ALL {
            validate_mimo_manifest(&builtin_mimo_manifest(b))
                .unwrap_or_else(|e| panic!("{b:?}: {e}"));
        }
        assert_eq!(BuiltinMimo::ALL[0].id(), DEFAULT_MIMO_LAW_ID);
        assert_eq!(default_mimo_choice().id, DEFAULT_MIMO_LAW_ID);
    }
}
