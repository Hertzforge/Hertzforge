//! The operator's choice of Sine law, and the factory it becomes.

use crate::wasm::{setting, WasmSineLawFactory};
use hertzforge_law_host::{validate, LawError};
use hertzforge_random_laws::{sine_builtin_manifest, ChoiceError, LawCatalog, LawChoice, LawWorld};
use hertzforge_sine::law::servo_blend::{ServoBlendFactory, ServoBlendParams};
use hertzforge_sine::SineLawFactory;
use std::sync::Arc;

/// The catalog id of the built-in Sine law, `servo_blend`: the default.
pub const DEFAULT_SINE_LAW_ID: &str = hertzforge_sine::law::SERVO_BLEND_ID;

/// The factory of the built-in law: the native `servo_blend`.
pub type BuiltinSineFactory = ServoBlendFactory;

/// The default law with its default parameters.
pub fn default_sine_choice() -> LawChoice {
    LawChoice::new(DEFAULT_SINE_LAW_ID, Vec::new())
}

/// The built-in's parameter values for `choice`, checked against its manifest
/// and its declared ranges, defaults for the rest.
pub fn builtin_sine_params(choice: &LawChoice) -> Result<ServoBlendParams, ChoiceError> {
    let params =
        validate::resolve_params(&sine_builtin_manifest(), &choice.params, 1).map_err(bad_param)?;
    let settings: Vec<_> = params
        .iter()
        .filter_map(|p| setting(&p.value).map(|v| (p.key.clone(), v)))
        .collect();
    ServoBlendParams::from_settings(&settings).map_err(|reason| ChoiceError::BadParam {
        key: reason.split_whitespace().next().unwrap_or("").to_string(),
        reason,
    })
}

fn bad_param(e: LawError) -> ChoiceError {
    match e {
        LawError::ParamInvalid { key, reason } => ChoiceError::BadParam { key, reason },
        other => ChoiceError::BadConfig(other.to_string()),
    }
}

/// Resolve `choice` against `catalog`: check its parameter values against the
/// law's manifest and return the factory a Sine run builds its law with: the
/// native built-in for `servo_blend`, a [`WasmSineLawFactory`] for a
/// `sine-single` law file.
///
/// Refusals: [`ChoiceError::UnknownLaw`] for an id the catalog does not hold,
/// [`ChoiceError::WrongTestType`] for a law of another world,
/// [`ChoiceError::BadParam`] for a value its declaration refuses.
pub fn resolve_sine(
    catalog: &LawCatalog,
    choice: &LawChoice,
) -> Result<Arc<dyn SineLawFactory>, ChoiceError> {
    if choice.id == DEFAULT_SINE_LAW_ID {
        let p = builtin_sine_params(choice)?;
        return Ok(Arc::new(BuiltinSineFactory { params: p }));
    }
    match catalog.entry(&choice.id) {
        None => Err(ChoiceError::UnknownLaw(choice.id.clone())),
        Some(entry) => match entry.world {
            LawWorld::Sine => {
                let law = catalog
                    .sine_law(&choice.id)
                    .ok_or_else(|| ChoiceError::UnknownLaw(choice.id.clone()))?;
                let params = law.resolve_params(&choice.params, 1).map_err(bad_param)?;
                Ok(Arc::new(WasmSineLawFactory::new(law.clone(), params)))
            }
            LawWorld::Single
            | LawWorld::Mimo
            | LawWorld::ShockClassical
            | LawWorld::ShockSrs
            | LawWorld::Twr
            | LawWorld::ShockMimo => Err(ChoiceError::WrongTestType {
                id: choice.id.clone(),
                law_is_for: entry.world.test_type(),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_law_host::{Param, ParamValue};
    use hertzforge_sine::CompressionPreset;

    #[test]
    fn the_built_in_resolves_with_its_parameters_and_other_worlds_are_refused() {
        let root = std::env::temp_dir().join(format!(
            "hertzforge-sine-laws-choice-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let cat = LawCatalog::open(&root).expect("an empty catalog opens");
        assert_eq!(DEFAULT_SINE_LAW_ID, "servo_blend");
        let f = resolve_sine(&cat, &default_sine_choice()).expect("the built-in resolves");
        let d = f.describe();
        assert_eq!((d.id.as_str(), d.sha256.as_deref()), ("servo_blend", None));
        let p = builtin_sine_params(&LawChoice::new(
            DEFAULT_SINE_LAW_ID,
            vec![
                Param::new("blend_w", ParamValue::Number(1.0)),
                Param::new("compression", ParamValue::Choice("fast".into())),
            ],
        ))
        .unwrap();
        assert_eq!(p.blend_w, 1.0);
        assert_eq!(p.compression, CompressionPreset::Fast);
        for (bad, key) in [
            (Param::new("blend_w", ParamValue::Number(1.5)), "blend_w"),
            (Param::new("nope", ParamValue::Number(1.0)), "nope"),
            (Param::new("kp", ParamValue::Toggle(true)), "kp"),
        ] {
            match resolve_sine(&cat, &LawChoice::new(DEFAULT_SINE_LAW_ID, vec![bad])) {
                Err(ChoiceError::BadParam { key: k, .. }) => assert_eq!(k, key),
                other => panic!("{key}: {:?}", other.err()),
            }
        }
        for (id, world) in [
            ("hinv", "single-output Random"),
            ("pinv_closed", "multiple-output Random"),
            ("fd_correct", "classical shock"),
            ("srs_ratio", "SRS shock"),
        ] {
            match resolve_sine(&cat, &LawChoice::new(id, Vec::new())) {
                Err(ChoiceError::WrongTestType { law_is_for, .. }) => assert_eq!(law_is_for, world),
                other => panic!("{id}: {:?}", other.err()),
            }
        }
        assert!(matches!(
            resolve_sine(&cat, &LawChoice::new("no_such_law", Vec::new())),
            Err(ChoiceError::UnknownLaw(_))
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
