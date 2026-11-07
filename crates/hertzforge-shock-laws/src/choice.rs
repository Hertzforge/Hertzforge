//! The operator's choice of classical shock law, and how it becomes the
//! factory the classical service builds the law with.

use hertzforge_random_laws::{ChoiceError, LawCatalog, LawChoice, LawWorld};
use hertzforge_shock_host::ClassicalLawFactory;
use std::sync::Arc;

/// The catalog id of the built-in classical law, `fd_correct`: the default.
pub const DEFAULT_CLASSICAL_LAW_ID: &str = hertzforge_shock_law_math::classical::ID;

/// The default law with its default parameters.
pub fn default_classical_choice() -> LawChoice {
    LawChoice::new(DEFAULT_CLASSICAL_LAW_ID, Vec::new())
}

/// Resolve `choice` against `catalog`: check its parameter values against the
/// law's manifest and return the factory a classical run builds its law with.
///
/// Refusals: [`ChoiceError::UnknownLaw`] for an id the catalog does not hold,
/// [`ChoiceError::WrongTestType`] for a Random law, [`ChoiceError::BadParam`]
/// for a value its declaration refuses.
pub fn resolve_classical(
    catalog: &LawCatalog,
    choice: &LawChoice,
) -> Result<Arc<dyn ClassicalLawFactory>, ChoiceError> {
    if choice.id == DEFAULT_CLASSICAL_LAW_ID {
        return Err(not_built());
    }
    match catalog.entry(&choice.id) {
        None => Err(ChoiceError::UnknownLaw(choice.id.clone())),
        Some(entry) => match entry.world {
            LawWorld::Single => Err(ChoiceError::WrongTestType {
                id: choice.id.clone(),
                law_is_for: "single-output Random",
            }),
            LawWorld::Mimo => Err(ChoiceError::WrongTestType {
                id: choice.id.clone(),
                law_is_for: "multiple-output Random",
            }),
        },
    }
}

fn not_built() -> ChoiceError {
    ChoiceError::BadConfig("classical shock control laws are not built yet".into())
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
    fn the_default_is_fd_correct_and_says_it_is_not_built_yet() {
        let (cat, root) = catalog();
        assert_eq!(default_classical_choice().id, "fd_correct");
        match resolve_classical(&cat, &default_classical_choice()) {
            Err(ChoiceError::BadConfig(m)) => assert!(m.contains("not built yet"), "{m}"),
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("resolved before the law is built"),
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
