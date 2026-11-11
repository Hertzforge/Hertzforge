//! The run's two law choices, and the factories they become.

use hertzforge_breakpoints::BreakpointTable;
use hertzforge_law_host::{Breakpoint, Param, ParamKind, ParamSpec, ParamValue};
use hertzforge_mixed::{LawPick, LawRole, MixedLawChoice, MixedLawFactories};
use hertzforge_random::RandomTestConfig;
use hertzforge_random_laws::{ChoiceError, LawCatalog, LawChoice};
use serde_json::Value;
use std::fmt;

/// Why one of the run's law choices cannot be run: the role, and the reason
/// its world's resolver gave.
#[derive(Debug, Clone, PartialEq)]
pub struct MixedLawError {
    /// The floor's law or the tones'.
    pub role: LawRole,
    /// Why (`WrongTestType` for a law of another world).
    pub error: ChoiceError,
}

impl fmt::Display for MixedLawError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let role = match self.role {
            LawRole::Floor => "the floor's law",
            LawRole::Tones => "the tones' law",
        };
        write!(f, "{role}: {}", self.error)
    }
}

impl std::error::Error for MixedLawError {}

/// `pick` as a catalog choice: its parameter values read by the kinds the
/// law's manifest declares. A key the manifest does not declare, or a value of
/// the wrong kind, is refused [`ChoiceError::BadParam`]; an id the catalog
/// does not hold, [`ChoiceError::UnknownLaw`].
pub fn law_choice(catalog: &LawCatalog, pick: &LawPick) -> Result<LawChoice, ChoiceError> {
    let entry = catalog
        .entry(&pick.id)
        .ok_or_else(|| ChoiceError::UnknownLaw(pick.id.clone()))?;
    let specs = &entry.manifest.params;
    let mut params = Vec::with_capacity(pick.params.len());
    for (key, v) in &pick.params {
        let spec = specs
            .iter()
            .find(|s| &s.key == key)
            .ok_or_else(|| ChoiceError::BadParam {
                key: key.clone(),
                reason: "not a parameter of this law".into(),
            })?;
        let value = param_value(spec, v).ok_or_else(|| ChoiceError::BadParam {
            key: key.clone(),
            reason: "not a value of the parameter's kind".into(),
        })?;
        params.push(Param::new(key.clone(), value));
    }
    Ok(LawChoice::new(pick.id.clone(), params))
}

fn param_value(spec: &ParamSpec, v: &Value) -> Option<ParamValue> {
    Some(match &spec.kind {
        ParamKind::Number(_) => ParamValue::Number(v.as_f64()?),
        ParamKind::Integer(_) => ParamValue::Integer(v.as_i64()?),
        ParamKind::Toggle(_) => ParamValue::Toggle(v.as_bool()?),
        ParamKind::Choice(_) => ParamValue::Choice(v.as_str()?.to_owned()),
        ParamKind::PerChannel(_) => ParamValue::PerChannel(
            v.as_array()?
                .iter()
                .map(Value::as_f64)
                .collect::<Option<Vec<_>>>()?,
        ),
        ParamKind::Curve(_) => ParamValue::Curve(
            v.as_array()?
                .iter()
                .map(|p| match p {
                    Value::Array(xy) if xy.len() == 2 => Some(Breakpoint {
                        frequency_hz: xy[0].as_f64()?,
                        value: xy[1].as_f64()?,
                    }),
                    Value::Object(o) => Some(Breakpoint {
                        frequency_hz: o.get("frequency_hz")?.as_f64()?,
                        value: o.get("value")?.as_f64()?,
                    }),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()?,
        ),
    })
}

/// Resolve the run's two laws against `catalog`: the floor's through Random's
/// resolver (a `single`-world law), the tones' through Sine's (a `sine`-world
/// law). `control_channels` is the run's control channel count, against which
/// a per-channel parameter is checked.
///
/// For the built-in floor law the settings its choice sets are returned in
/// [`MixedLawFactories::floor_hinv`], since the core reads some of them too.
/// A law of the wrong world for its role is refused
/// [`ChoiceError::WrongTestType`] naming the world it is for.
pub fn resolve_mixed(
    catalog: &LawCatalog,
    choice: &MixedLawChoice,
    control_channels: usize,
) -> Result<MixedLawFactories, MixedLawError> {
    let floor_err = |error| MixedLawError {
        role: LawRole::Floor,
        error,
    };
    let tones_err = |error| MixedLawError {
        role: LawRole::Tones,
        error,
    };
    let floor_choice = law_choice(catalog, &choice.floor).map_err(floor_err)?;
    // Random's resolver writes the built-in's settings into a run's
    // configuration; a scratch one with the run's control channel count
    // carries them out.
    let mut scratch = RandomTestConfig::siso(BreakpointTable::default(), 0);
    scratch.control.channels = (0..control_channels.max(1) as u32)
        .map(|i| hertzforge_random::ControlChannel {
            channel_index: i,
            weight: 1.0,
        })
        .collect();
    let floor = catalog
        .resolve(&floor_choice, &mut scratch)
        .map_err(floor_err)?;
    // The built-in tone law reads through a narrow tracker in a mixed run, so
    // its compression defaults to the one that holds there, not Sine's.
    let mut tones_pick = choice.tones.clone();
    if tones_pick.id == hertzforge_mixed::DEFAULT_TONE_LAW_ID
        && !tones_pick.params.contains_key("compression")
    {
        let p = &mut tones_pick.params;
        p.insert("compression".into(), Value::from("custom"));
        p.entry("ki")
            .or_insert(Value::from(hertzforge_mixed::service::MIXED_TONE_KI));
        p.entry("max_rate_db_per_s").or_insert(Value::from(
            hertzforge_mixed::service::MIXED_TONE_MAX_RATE_DB_PER_S,
        ));
    }
    let tone_choice = law_choice(catalog, &tones_pick).map_err(tones_err)?;
    let tones = hertzforge_sine_laws::resolve_sine(catalog, &tone_choice).map_err(tones_err)?;
    Ok(MixedLawFactories {
        floor,
        floor_hinv: scratch.hinv,
        tones,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(tag: &str) -> (LawCatalog, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "hertzforge-mixed-laws-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        (
            LawCatalog::open(&root).expect("an empty catalog opens"),
            root,
        )
    }

    #[test]
    fn the_built_ins_resolve_and_the_floor_law_s_settings_come_back() {
        let (cat, root) = catalog("builtin");
        let l = resolve_mixed(&cat, &MixedLawChoice::default(), 1).unwrap();
        assert_eq!(l.tones.describe().id, "servo_blend");
        // The tones take the mixed compression unless one is chosen.
        let params = l.tones.describe().params;
        let get = |k: &str| {
            params
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        };
        use hertzforge_sine::law::ParamSetting;
        assert_eq!(
            get("compression"),
            Some(ParamSetting::Choice("custom".into()))
        );
        assert_eq!(
            get("ki"),
            Some(ParamSetting::Number(
                hertzforge_mixed::service::MIXED_TONE_KI
            ))
        );
        let mut slow = MixedLawChoice::default();
        slow.tones
            .params
            .insert("compression".into(), serde_json::json!("slow"));
        let l = resolve_mixed(&cat, &slow, 1).unwrap();
        let params = l.tones.describe().params;
        assert_eq!(
            params
                .iter()
                .find(|(k, _)| k == "compression")
                .map(|(_, v)| v.clone()),
            Some(ParamSetting::Choice("slow".into()))
        );
        assert_eq!(l.floor_hinv, hertzforge_random::HinvConfig::default());
        // A setting given for the floor's built-in reaches its settings.
        let mut choice = MixedLawChoice::default();
        choice
            .floor
            .params
            .insert("max_correction_db_per_iter".into(), serde_json::json!(1.0));
        let l = resolve_mixed(&cat, &choice, 1).unwrap();
        assert_eq!(l.floor_hinv.max_correction_db_per_iter, 1.0);
        assert_ne!(l.floor_hinv, hertzforge_random::HinvConfig::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_law_of_the_wrong_world_is_refused_by_name_for_its_role() {
        let (cat, root) = catalog("world");
        let c = MixedLawChoice {
            floor: LawPick::new("servo_blend"),
            ..Default::default()
        };
        let e = resolve_mixed(&cat, &c, 1).err().unwrap();
        assert_eq!(e.role, LawRole::Floor);
        assert!(matches!(
            e.error,
            ChoiceError::WrongTestType {
                law_is_for: "Sine",
                ..
            }
        ));
        let c = MixedLawChoice {
            tones: LawPick::new("hinv"),
            ..Default::default()
        };
        let e = resolve_mixed(&cat, &c, 1).err().unwrap();
        assert_eq!(e.role, LawRole::Tones);
        assert!(matches!(
            e.error,
            ChoiceError::WrongTestType {
                law_is_for: "single-output Random",
                ..
            }
        ));
        let c = MixedLawChoice {
            tones: LawPick::new("no_such_law"),
            ..Default::default()
        };
        assert!(matches!(
            resolve_mixed(&cat, &c, 1).err().unwrap().error,
            ChoiceError::UnknownLaw(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_parameter_the_law_does_not_declare_or_of_the_wrong_kind_is_refused() {
        let (cat, root) = catalog("params");
        let mut c = MixedLawChoice::default();
        c.tones.params.insert("nope".into(), serde_json::json!(1.0));
        match resolve_mixed(&cat, &c, 1).err().unwrap() {
            MixedLawError {
                role: LawRole::Tones,
                error: ChoiceError::BadParam { key, .. },
            } => assert_eq!(key, "nope"),
            other => panic!("{other:?}"),
        }
        let mut c = MixedLawChoice::default();
        c.tones
            .params
            .insert("blend_w".into(), serde_json::json!("high"));
        assert!(matches!(
            resolve_mixed(&cat, &c, 1).err().unwrap().error,
            ChoiceError::BadParam { .. }
        ));
        // Out of range is the resolver's own refusal.
        let mut c = MixedLawChoice::default();
        c.tones
            .params
            .insert("blend_w".into(), serde_json::json!(1.5));
        assert!(matches!(
            resolve_mixed(&cat, &c, 1).err().unwrap().error,
            ChoiceError::BadParam { .. }
        ));
        let mut c = MixedLawChoice::default();
        c.tones
            .params
            .insert("blend_w".into(), serde_json::json!(1.0));
        assert!(resolve_mixed(&cat, &c, 1).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }
}
