//! The operator's choice of TWR law, and the factory it becomes.

use hertzforge_random_laws::{ChoiceError, LawCatalog, LawChoice, LawWorld};
use hertzforge_twr::TwrLawFactory;
use std::sync::Arc;

/// The catalog id of the built-in TWR law, `twr_correct`: the default.
pub const DEFAULT_TWR_LAW_ID: &str = hertzforge_twr::control::TWR_CORRECT_ID;

/// The default law with its default parameters.
pub fn default_twr_choice() -> LawChoice {
    LawChoice::new(DEFAULT_TWR_LAW_ID, Vec::new())
}

/// Resolve `choice` against `catalog`: check its parameter values against the
/// law's manifest and return the factory a TWR run builds its law with.
///
/// Refusals: [`ChoiceError::UnknownLaw`] for an id the catalog does not hold,
/// [`ChoiceError::WrongTestType`] for a law of another world,
/// [`ChoiceError::BadParam`] for a value its declaration refuses. Neither the
/// built-in law nor a `twr-single` law file is built yet; both are refused
/// ([`ChoiceError::BadConfig`]).
pub fn resolve_twr(
    catalog: &LawCatalog,
    choice: &LawChoice,
) -> Result<Arc<dyn TwrLawFactory>, ChoiceError> {
    if choice.id == DEFAULT_TWR_LAW_ID {
        return Err(ChoiceError::BadConfig(
            "the built-in TWR law is not built yet".into(),
        ));
    }
    match catalog.entry(&choice.id) {
        None => Err(ChoiceError::UnknownLaw(choice.id.clone())),
        Some(entry) => match entry.world {
            LawWorld::Twr => Err(ChoiceError::BadConfig(
                "TWR law files are not built yet".into(),
            )),
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

    #[test]
    fn nothing_resolves_until_the_built_in_is_built_and_other_worlds_are_refused() {
        let root =
            std::env::temp_dir().join(format!("hertzforge-twr-laws-choice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cat = LawCatalog::open(&root).expect("an empty catalog opens");
        assert_eq!(DEFAULT_TWR_LAW_ID, "twr_correct");
        match resolve_twr(&cat, &default_twr_choice()) {
            Err(ChoiceError::BadConfig(m)) => assert!(m.contains("not built yet"), "{m}"),
            other => panic!("the built-in resolved: {:?}", other.err()),
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
