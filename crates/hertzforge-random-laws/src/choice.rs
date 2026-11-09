//! The operator's choice of control law for a run, and how it becomes the
//! factory the run service builds the law with.

use crate::adapter::WasmLawFactory;
use crate::builtin::{BuiltinLaw, DEFAULT_LAW_ID};
use crate::catalog::{LawCatalog, LawWorld};
use crate::ratio::NativeRatioFactory;
use hertzforge_law_host::validate::resolve_params;
use hertzforge_law_host::{AnyLaw, LawError, Param, ParamValue};
use hertzforge_law_ratio_math::RatioParams;
use hertzforge_random::service::config::RandomTestConfig;
use hertzforge_random::service::law::{HinvLawFactory, LawFactory};
use std::fmt;
use std::sync::Arc;

/// A law, by its catalog id, and the parameter values the operator set. Values
/// left out take the law's declared defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct LawChoice {
    /// The catalog id: `hinv`, `ratio`, or an imported law file's SHA-256.
    pub id: String,
    /// Parameter values by key.
    pub params: Vec<Param>,
}

impl LawChoice {
    /// The law `id` with `params`.
    pub fn new(id: impl Into<String>, params: Vec<Param>) -> Self {
        LawChoice {
            id: id.into(),
            params,
        }
    }
}

impl Default for LawChoice {
    /// The default law with its default parameters.
    fn default() -> Self {
        LawChoice::new(DEFAULT_LAW_ID, Vec::new())
    }
}

/// Why a choice of law cannot be run.
#[derive(Debug, Clone, PartialEq)]
pub enum ChoiceError {
    /// No law in the catalog has this id.
    UnknownLaw(String),
    /// A parameter value does not fit its declaration (an unknown key, the
    /// wrong kind, out of range).
    BadParam {
        /// The key as given.
        key: String,
        /// Why it was refused.
        reason: String,
    },
    /// The values fit their declarations but not the law together.
    BadConfig(String),
    /// The law is for the other test type: a multiple-output law chosen for a
    /// single-output run, or the other way round.
    WrongTestType {
        /// The law's id.
        id: String,
        /// What the law is for, in words.
        law_is_for: &'static str,
    },
}

impl fmt::Display for ChoiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChoiceError::UnknownLaw(id) => write!(f, "there is no control law `{id}`"),
            ChoiceError::BadParam { key, reason } => {
                write!(f, "control law parameter `{key}`: {reason}")
            }
            ChoiceError::BadConfig(s) => write!(f, "control law settings: {s}"),
            ChoiceError::WrongTestType { id, law_is_for } => {
                write!(f, "control law `{id}` is for {law_is_for}")
            }
        }
    }
}

impl std::error::Error for ChoiceError {}

fn number(p: &Param) -> Result<f64, ChoiceError> {
    match p.value {
        ParamValue::Number(v) => Ok(v),
        ParamValue::Integer(v) => Ok(v as f64),
        _ => Err(ChoiceError::BadParam {
            key: p.key.clone(),
            reason: "not a number".into(),
        }),
    }
}

impl LawCatalog {
    /// Resolve `choice` for a run configured by `cfg`: find the law, check the
    /// parameter values against its manifest (completing them with defaults),
    /// and return the factory the run service builds the law with
    /// ([`hertzforge_random::RandomService::with_law_factory`]).
    ///
    /// For `hinv` every declared value (a value left out is the declared
    /// default, not what `cfg.hinv` held) is written into `cfg.hinv`, which the
    /// factory builds from: the run core reads some of those settings too (the
    /// divergence count, the notch's gain floor, the change cap's floor), so
    /// they must be the law's. Any other law leaves `cfg` as it is. Nothing in
    /// `cfg` changes when the choice is refused.
    pub fn resolve(
        &self,
        choice: &LawChoice,
        cfg: &mut RandomTestConfig,
    ) -> Result<Arc<dyn LawFactory>, ChoiceError> {
        let entry = self
            .entry(&choice.id)
            .ok_or_else(|| ChoiceError::UnknownLaw(choice.id.clone()))?;
        if entry.world != LawWorld::Single {
            return Err(ChoiceError::WrongTestType {
                id: choice.id.clone(),
                law_is_for: entry.world.test_type(),
            });
        }
        let params = resolve_params(&entry.manifest, &choice.params, cfg.control.channels.len())
            .map_err(|e| match e {
                LawError::ParamInvalid { key, reason } => ChoiceError::BadParam { key, reason },
                other => ChoiceError::BadConfig(other.to_string()),
            })?;
        match BuiltinLaw::from_id(&choice.id) {
            Some(BuiltinLaw::Hinv) => {
                let mut hinv = cfg.hinv.clone();
                for p in &params {
                    hinv.set_knob(&p.key, number(p)?)
                        .map_err(|e| ChoiceError::BadParam {
                            key: p.key.clone(),
                            reason: e.to_string(),
                        })?;
                }
                hinv.validate()
                    .map_err(|e| ChoiceError::BadConfig(e.to_string()))?;
                cfg.hinv = hinv;
                Ok(Arc::new(HinvLawFactory))
            }
            Some(BuiltinLaw::Ratio) => {
                let mut rp = RatioParams::default();
                for p in &params {
                    rp.set(&p.key, number(p)?)
                        .map_err(|reason| ChoiceError::BadParam {
                            key: p.key.clone(),
                            reason,
                        })?;
                }
                Ok(Arc::new(NativeRatioFactory::new(rp)))
            }
            None => match self.imported_law(&choice.id).map(|i| &i.law) {
                Some(AnyLaw::Single(law)) => Ok(Arc::new(WasmLawFactory::new(law.clone(), params))),
                Some(AnyLaw::Mimo(_)) => Err(ChoiceError::WrongTestType {
                    id: choice.id.clone(),
                    law_is_for: "multiple-output Random",
                }),
                Some(AnyLaw::ShockClassical(_)) => Err(ChoiceError::WrongTestType {
                    id: choice.id.clone(),
                    law_is_for: "classical shock",
                }),
                Some(AnyLaw::ShockSrs(_)) => Err(ChoiceError::WrongTestType {
                    id: choice.id.clone(),
                    law_is_for: "SRS shock",
                }),
                Some(AnyLaw::Sine(_)) => Err(ChoiceError::WrongTestType {
                    id: choice.id.clone(),
                    law_is_for: "Sine",
                }),
                Some(AnyLaw::Twr(_)) => Err(ChoiceError::WrongTestType {
                    id: choice.id.clone(),
                    law_is_for: "TWR",
                }),
                None => Err(ChoiceError::UnknownLaw(choice.id.clone())),
            },
        }
    }
}
