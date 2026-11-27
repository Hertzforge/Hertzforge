//! The choice of a MIMO Sine law: the built-in laws by name, law files of the
//! `sine-mimo` world by catalog id or by path, the catalog entries and the
//! facts the law picker shows, and side-by-side runs that show a law file and
//! its native form give the same drives.
//!
//! - [`resolve`]: a catalog id to the factory a run builds its law with. A
//!   built-in id (`matrix`, `paired`, `steepest_descent`, `secant`, `ratio`)
//!   goes to [`resolve_builtin_with`], its native form over the SDK's
//!   built-in constructor (`hertzforge_mimo_sine::laws`, the engine's own);
//!   any other is a `sine-mimo` law file in
//!   the catalog, run through the law host ([`wasm`]).
//! - [`load_law_file`]: a law file by path, as a contributor tries one.
//! - [`entries`]: the world's catalog entries, built-ins first, each with the
//!   shapes, test types and amplitude-only flag the shared entry lacks;
//!   [`all_facts`]: what the law picker shows, with each built-in's limits.
//! - [`parity`]: two laws over one recorded sequence.
//! - [`check`]: the law check: its cases and their requirements, the runner
//!   that plays a case through the engine ([`run_check_case`]) and the judge;
//!   [`check_law`] runs every case a law accepts.

pub mod check;
pub mod choice;
pub mod facts;
pub mod parity;
pub mod wasm;

pub use check::{
    cases_for, check_law, check_passed, check_record, judge, plan_setup, run_check, run_check_case,
    run_plan, CaseMeasure, CaseResult, CaseVerdict, CheckNotBuilt, CheckRunError,
    MimoSineCheckCase, Requirement, RunPlan, StationSpec, MIMO_SINE_CHECK_VERSION,
};
pub use choice::{
    builtin_decl, load_law_file, resolve, resolve_builtin, resolve_builtin_with, resolve_for_run,
    ResolveError,
};
pub use facts::{
    all_facts, builtin_entry, builtin_facts, entries, file_facts, limits, LawFacts, SineMimoEntry,
};
pub use hertzforge_mimo_sine::laws::BuiltinFactory;
pub use parity::{compare_laws, ParityReport, RecordedInputs};
pub use wasm::{
    sine_mimo_law_fault, WasmSineMimoLaw, WasmSineMimoLawFactory, DEFAULT_BUILD_BUDGET_S,
};

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
    use hertzforge_law_host::{Param, ParamValue};
    use hertzforge_law_sdk::sine_mimo::builtin;
    use hertzforge_mimo_sine::TestType;

    #[test]
    fn mimo_sine_the_built_in_laws_are_named_and_the_world_is_sine_mimo() {
        assert_eq!(WORLD.as_str(), "sine_mimo");
        for id in [
            "matrix",
            "paired",
            "steepest_descent",
            "secant",
            "ratio",
            "followed_poles",
        ] {
            assert!(is_builtin(id), "{id}");
        }
        assert!(!is_builtin("inverse"));
        assert_eq!(hertzforge_mimo_sine::DEFAULT_LAW_ID, "matrix");
        // The SDK's declarations are the core's built-in ids, in its order.
        let ids: Vec<&str> = builtin::ALL.iter().map(|d| d.id).collect();
        assert_eq!(ids, BUILTIN_LAW_IDS);
    }

    #[test]
    fn mimo_sine_a_builtin_resolves_to_its_native_law_by_name() {
        for d in builtin::ALL {
            let f = resolve_builtin(d.id).unwrap();
            let desc = f.describe();
            assert_eq!(desc.id, d.id);
            assert_eq!(desc.sha256, None);
            assert_eq!(desc.params.len(), d.params.len(), "{}", d.id);
            let m = f.manifest();
            assert_eq!(m.values, d.value_keys(), "{}", d.id);
            assert_eq!(m.shapes.len(), 3, "{}", d.id);
        }
        let m = resolve_builtin("matrix").unwrap().manifest();
        assert_eq!(m.test_types, TestType::ALL);
        assert!(m.amplitude_only);
        let m = resolve_builtin("secant").unwrap().manifest();
        assert_eq!(m.test_types, [TestType::Dwell]);
        assert!(!m.amplitude_only);
        assert_eq!(
            resolve_builtin("nope").err(),
            Some(ResolveError::Unknown("nope".into()))
        );
    }

    #[test]
    fn mimo_sine_a_set_parameter_reaches_the_law_and_a_stray_or_bad_one_is_refused() {
        let p = |key: &str, value| Param {
            key: key.into(),
            value,
        };
        let f = resolve_builtin_with("matrix", &[p("carry", ParamValue::Number(0.25))]).unwrap();
        let carry = f
            .describe()
            .params
            .into_iter()
            .find(|(k, _)| k == "carry")
            .unwrap()
            .1;
        assert_eq!(carry, hertzforge_mimo_sine::law::ParamSetting::Number(0.25));
        for bad in [
            p("gain", ParamValue::Number(0.5)),
            p("carry", ParamValue::Number(1.5)),
            p("model", ParamValue::Choice("other".into())),
        ] {
            let e = resolve_builtin_with("matrix", &[bad]).err().unwrap();
            assert!(matches!(e, ResolveError::BadParams(_)), "{e:?}");
        }
    }

    #[test]
    fn mimo_sine_every_builtin_states_its_limits() {
        for d in builtin::ALL {
            let l = limits(d.id).unwrap();
            assert!(!l.is_empty());
            assert_eq!(builtin_facts(&d).limits, Some(l));
        }
        assert!(limits("matrix")
            .unwrap()
            .contains("a quarter of its half-power bandwidth"));
        assert_eq!(limits("inverse"), None);
    }
}
