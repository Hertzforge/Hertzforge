//! The operator's choice of classical shock law, and how it becomes the
//! factory the classical service builds the law with.

use crate::wasm::WasmClassicalLawFactory;
use hertzforge_law_host::{validate, LawError, ParamValue};
use hertzforge_random_laws::{
    classical_builtin_manifest, ChoiceError, LawCatalog, LawChoice, LawWorld,
};
use hertzforge_shock_host::{ClassicalLawFactory, FdCorrectFactory};
use hertzforge_shock_law_math::classical::FdCorrectParams;
use std::sync::Arc;

/// The catalog id of the built-in classical law, `fd_correct`: the default.
pub const DEFAULT_CLASSICAL_LAW_ID: &str = hertzforge_shock_law_math::classical::ID;

/// The default law with its default parameters.
pub fn default_classical_choice() -> LawChoice {
    LawChoice::new(DEFAULT_CLASSICAL_LAW_ID, Vec::new())
}

/// Resolve `choice` against `catalog`: check its parameter values against the
/// law's manifest and return the factory a classical run builds its law with:
/// the native built-in for `fd_correct`, a [`WasmClassicalLawFactory`] for a
/// `shock-classical` law file.
///
/// Refusals: [`ChoiceError::UnknownLaw`] for an id the catalog does not hold,
/// [`ChoiceError::WrongTestType`] for a Random law, [`ChoiceError::BadParam`]
/// for a value its declaration refuses.
pub fn resolve_classical(
    catalog: &LawCatalog,
    choice: &LawChoice,
) -> Result<Arc<dyn ClassicalLawFactory>, ChoiceError> {
    let bad = |e: LawError| match e {
        LawError::ParamInvalid { key, reason } => ChoiceError::BadParam { key, reason },
        other => ChoiceError::BadConfig(other.to_string()),
    };
    if choice.id == DEFAULT_CLASSICAL_LAW_ID {
        let params = validate::resolve_params(&classical_builtin_manifest(), &choice.params, 1)
            .map_err(bad)?;
        let mut p = FdCorrectParams::default();
        for param in &params {
            if let ParamValue::Number(v) = param.value {
                p.set(&param.key, v)
                    .map_err(|reason| ChoiceError::BadParam {
                        key: param.key.clone(),
                        reason,
                    })?;
            }
        }
        return Ok(Arc::new(FdCorrectFactory::new(p)));
    }
    match catalog.entry(&choice.id) {
        None => Err(ChoiceError::UnknownLaw(choice.id.clone())),
        Some(entry) => match entry.world {
            LawWorld::ShockClassical => {
                let law = catalog
                    .classical_law(&choice.id)
                    .ok_or_else(|| ChoiceError::UnknownLaw(choice.id.clone()))?;
                let params = law.resolve_params(&choice.params).map_err(bad)?;
                Ok(Arc::new(WasmClassicalLawFactory::new(law.clone(), params)))
            }
            world => Err(ChoiceError::WrongTestType {
                id: choice.id.clone(),
                law_is_for: world.test_type(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> (LawCatalog, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "hertzforge-shock-laws-choice-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        (
            LawCatalog::open(&root).expect("an empty catalog opens"),
            root,
        )
    }

    #[test]
    fn a_random_law_is_refused_for_classical_shock_by_its_world() {
        let (cat, root) = catalog();
        for (id, world) in [
            ("hinv", "single-output Random"),
            ("pinv_closed", "multiple-output Random"),
        ] {
            match resolve_classical(&cat, &LawChoice::new(id, Vec::new())) {
                Err(ChoiceError::WrongTestType { law_is_for, .. }) => assert_eq!(law_is_for, world),
                Err(e) => panic!("{id}: {e}"),
                Ok(_) => panic!("{id} resolved for classical shock"),
            }
        }
        assert!(matches!(
            resolve_classical(&cat, &LawChoice::new("no_such_law", Vec::new())),
            Err(ChoiceError::UnknownLaw(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_default_is_fd_correct_with_checked_parameters() {
        use hertzforge_law_host::Param;
        let (cat, root) = catalog();
        assert_eq!(default_classical_choice().id, "fd_correct");
        let f = resolve_classical(&cat, &default_classical_choice()).unwrap();
        let d = f.describe();
        assert_eq!((d.id.as_str(), d.sha256), ("fd_correct", None));
        assert_eq!(d.params["beta"], 0.5);
        let choice = LawChoice::new(
            "fd_correct",
            vec![
                Param::new("beta", ParamValue::Number(0.25)),
                Param::new("initial_backoff_db", ParamValue::Number(3.0)),
            ],
        );
        let d = resolve_classical(&cat, &choice).unwrap().describe();
        assert_eq!(
            (
                d.params["beta"].as_f64(),
                d.params["initial_backoff_db"].as_f64()
            ),
            (Some(0.25), Some(3.0))
        );
        for (key, v) in [("beta", 2.0), ("gain", 1.0)] {
            let c = LawChoice::new("fd_correct", vec![Param::new(key, ParamValue::Number(v))]);
            assert!(
                matches!(
                    resolve_classical(&cat, &c),
                    Err(ChoiceError::BadParam { .. })
                ),
                "{key}"
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
