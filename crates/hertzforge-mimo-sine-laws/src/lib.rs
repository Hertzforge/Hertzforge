//! The choice of a MIMO Sine law: the built-in laws by name and law files of
//! the `sine-mimo` world by catalog id.
//!
//! This version holds the names only: which ids are built in, and the world a
//! law file must be of. Resolving an id to the factory a run builds its law
//! with, the facts the law picker shows, the law check and the parity runs
//! are not in this version.

use hertzforge_mimo_sine::BUILTIN_LAW_IDS;
use hertzforge_random_laws::LawWorld;

/// The catalog world of a MIMO Sine law.
pub const WORLD: LawWorld = LawWorld::SineMimo;

/// Whether `id` names a built-in MIMO Sine law.
pub fn is_builtin(id: &str) -> bool {
    BUILTIN_LAW_IDS.contains(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_sine_the_built_in_laws_are_named_and_the_world_is_sine_mimo() {
        assert_eq!(WORLD.as_str(), "sine_mimo");
        for id in ["matrix", "paired", "steepest_descent", "secant", "ratio"] {
            assert!(is_builtin(id), "{id}");
        }
        assert!(!is_builtin("inverse"));
        assert_eq!(hertzforge_mimo_sine::DEFAULT_LAW_ID, "matrix");
    }
}
