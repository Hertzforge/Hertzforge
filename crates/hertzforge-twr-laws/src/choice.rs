//! The operator's choice of TWR law, and the factory it becomes.

use crate::wasm::WasmTwrLawFactory;
use hertzforge_law_host::{validate, LawError, ParamValue};
use hertzforge_random_laws::{twr_builtin_manifest, ChoiceError, LawCatalog, LawChoice, LawWorld};
use hertzforge_shock_law_math::twr::TwrCorrectParams;
use hertzforge_twr::{TwrCorrectFactory, TwrLawFactory};
use std::sync::Arc;

/// The catalog id of the built-in TWR law, `twr_correct`: the default.
pub const DEFAULT_TWR_LAW_ID: &str = hertzforge_twr::control::TWR_CORRECT_ID;

/// The factory of the built-in law: the native `twr_correct`, the core's own.
pub type BuiltinTwrFactory = TwrCorrectFactory;

/// The default law with its default parameters.
pub fn default_twr_choice() -> LawChoice {
    LawChoice::new(DEFAULT_TWR_LAW_ID, Vec::new())
}

fn bad_param(e: LawError) -> ChoiceError {
    match e {
        LawError::ParamInvalid { key, reason } => ChoiceError::BadParam { key, reason },
        other => ChoiceError::BadConfig(other.to_string()),
    }
}

/// The built-in's parameter values for `choice`, checked against its manifest
/// and its declared ranges (`beta` 0.05 to 1, `max_db_per_iter` 1 to 9,
/// `initial_backoff_db` 0 to 12), defaults for the rest.
pub fn builtin_twr_params(choice: &LawChoice) -> Result<TwrCorrectParams, ChoiceError> {
    let params =
        validate::resolve_params(&twr_builtin_manifest(), &choice.params, 1).map_err(bad_param)?;
    let mut p = TwrCorrectParams::default();
    for param in &params {
        let ParamValue::Number(v) = param.value else {
            return Err(ChoiceError::BadParam {
                key: param.key.clone(),
                reason: "the built-in's parameters are numbers".into(),
            });
        };
        p.set(&param.key, v)
            .map_err(|reason| ChoiceError::BadParam {
                key: param.key.clone(),
                reason,
            })?;
    }
    Ok(p)
}

/// Resolve `choice` against `catalog`: check its parameter values against the
/// law's manifest and return the factory a TWR run builds its law with: the
/// native built-in for `twr_correct`, a [`WasmTwrLawFactory`] for a
/// `twr-single` law file.
///
/// Refusals: [`ChoiceError::UnknownLaw`] for an id the catalog does not hold,
/// [`ChoiceError::WrongTestType`] for a law of another world,
/// [`ChoiceError::BadParam`] for a value its declaration refuses.
pub fn resolve_twr(
    catalog: &LawCatalog,
    choice: &LawChoice,
) -> Result<Arc<dyn TwrLawFactory>, ChoiceError> {
    if choice.id == DEFAULT_TWR_LAW_ID {
        return Ok(Arc::new(BuiltinTwrFactory::new(builtin_twr_params(
            choice,
        )?)));
    }
    match catalog.entry(&choice.id) {
        None => Err(ChoiceError::UnknownLaw(choice.id.clone())),
        Some(entry) => match entry.world {
            LawWorld::Twr => {
                let law = catalog
                    .twr_law(&choice.id)
                    .ok_or_else(|| ChoiceError::UnknownLaw(choice.id.clone()))?;
                let params = law.resolve_params(&choice.params).map_err(bad_param)?;
                Ok(Arc::new(WasmTwrLawFactory::new(law.clone(), params)))
            }
            LawWorld::Single
            | LawWorld::Mimo
            | LawWorld::ShockClassical
            | LawWorld::ShockSrs
            | LawWorld::Sine => Err(ChoiceError::WrongTestType {
                id: choice.id.clone(),
                law_is_for: entry.world.test_type(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_law_host::Param;

    #[test]
    fn the_built_in_resolves_with_its_parameters_and_other_worlds_are_refused() {
        let root =
            std::env::temp_dir().join(format!("hertzforge-twr-laws-choice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cat = LawCatalog::open(&root).expect("an empty catalog opens");
        assert_eq!(DEFAULT_TWR_LAW_ID, "twr_correct");
        let f = resolve_twr(&cat, &default_twr_choice()).expect("the built-in resolves");
        let d = f.describe();
        assert_eq!((d.id.as_str(), d.sha256.as_deref()), ("twr_correct", None));
        assert_eq!(d.params["beta"], 0.5);
        let p = builtin_twr_params(&LawChoice::new(
            DEFAULT_TWR_LAW_ID,
            vec![
                Param::new("beta", ParamValue::Number(0.25)),
                Param::new("initial_backoff_db", ParamValue::Number(3.0)),
            ],
        ))
        .unwrap();
        assert_eq!(
            (p.beta, p.max_db_per_iter, p.initial_backoff_db),
            (0.25, 6.0, 3.0)
        );
        for (bad, key) in [
            (Param::new("beta", ParamValue::Number(1.5)), "beta"),
            (
                Param::new("max_db_per_iter", ParamValue::Number(0.5)),
                "max_db_per_iter",
            ),
            (Param::new("nope", ParamValue::Number(1.0)), "nope"),
            (Param::new("beta", ParamValue::Toggle(true)), "beta"),
        ] {
            match resolve_twr(&cat, &LawChoice::new(DEFAULT_TWR_LAW_ID, vec![bad])) {
                Err(ChoiceError::BadParam { key: k, .. }) => assert_eq!(k, key),
                other => panic!("{key}: {:?}", other.err()),
            }
        }
        for (id, world) in [
            ("hinv", "single-output Random"),
            ("pinv_closed", "multiple-output Random"),
            ("fd_correct", "classical shock"),
            ("srs_ratio", "SRS shock"),
            ("servo_blend", "Sine"),
        ] {
            match resolve_twr(&cat, &LawChoice::new(id, Vec::new())) {
                Err(ChoiceError::WrongTestType { law_is_for, .. }) => assert_eq!(law_is_for, world),
                other => panic!("{id}: {:?}", other.err()),
            }
        }
        assert!(matches!(
            resolve_twr(&cat, &LawChoice::new("no_such_law", Vec::new())),
            Err(ChoiceError::UnknownLaw(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
