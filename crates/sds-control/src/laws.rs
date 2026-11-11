//! The control-law catalog on the wire (schema/PROTOCOL.md, "Control Laws"):
//! `laws.list` and `laws.import`, for single-output and multiple-output Random
//! and classical shock alike. The catalog is the controller's own
//! (`hertzforge-random-laws`): the built-in laws, and the law files imported into
//! the local data root.

use crate::protocol::{num, ErrorCode, MethodError};
use hertzforge_law_host::{DiagnosticKind, ParamKind, ParamSpec};
use hertzforge_random_laws::{
    may_arm_on_hardware, CatalogEntry, CatalogError, CheckStatus, LawCatalog, LawWorld,
};
use hertzforge_sdm::law::{MimoInput, MimoShape};
use serde_json::{json, Map, Value};
use std::path::Path;

/// A world on the wire.
pub fn world_name(w: LawWorld) -> &'static str {
    match w {
        LawWorld::Single => "single",
        LawWorld::Mimo => "mimo",
        LawWorld::ShockClassical => "shock_classical",
        LawWorld::ShockSrs => "shock_srs",
        LawWorld::Sine => "sine",
        LawWorld::Twr => "twr",
    }
}

fn shape_name(s: MimoShape) -> &'static str {
    crate::mimo::setup::shape_name(s)
}

fn input_name(i: MimoInput) -> &'static str {
    match i {
        MimoInput::LastDrive => "last_drive",
        MimoInput::ResponseDriveCsd => "response_drive_csd",
        MimoInput::SysidResponse => "sysid_response",
        MimoInput::SysidDrive => "sysid_drive",
        MimoInput::SysidNoise => "sysid_noise",
    }
}

fn layout_name(k: DiagnosticKind) -> &'static str {
    match k {
        DiagnosticKind::Scalar => "scalar",
        DiagnosticKind::PerLine => "per_line",
        DiagnosticKind::PerControlLine => "per_control_line",
        DiagnosticKind::PerDriveLine => "per_drive_line",
    }
}

/// One declared parameter: its kind, limits and default, so a form is built from
/// it without knowing the law.
pub fn param_json(p: &ParamSpec) -> Value {
    let mut m = Map::new();
    m.insert("key".into(), Value::from(p.key.as_str()));
    m.insert("label".into(), Value::from(p.label.as_str()));
    m.insert("help".into(), Value::from(p.help.as_str()));
    match &p.kind {
        ParamKind::Number(s) | ParamKind::PerChannel(s) => {
            let kind = if matches!(p.kind, ParamKind::Number(_)) {
                "number"
            } else {
                "per_channel"
            };
            m.insert("kind".into(), Value::from(kind));
            m.insert("unit".into(), Value::from(s.unit.as_str()));
            m.insert("min".into(), num(s.min));
            m.insert("max".into(), num(s.max));
            m.insert("step".into(), num(s.step));
            m.insert("default".into(), num(s.default));
        }
        ParamKind::Integer(s) => {
            m.insert("kind".into(), Value::from("integer"));
            m.insert("unit".into(), Value::from(s.unit.as_str()));
            m.insert("min".into(), Value::from(s.min));
            m.insert("max".into(), Value::from(s.max));
            m.insert("step".into(), Value::from(s.step));
            m.insert("default".into(), Value::from(s.default));
        }
        ParamKind::Toggle(d) => {
            m.insert("kind".into(), Value::from("toggle"));
            m.insert("default".into(), Value::from(*d));
        }
        ParamKind::Choice(s) => {
            m.insert("kind".into(), Value::from("choice"));
            m.insert("options".into(), Value::from(s.options.clone()));
            m.insert("default".into(), Value::from(s.default.as_str()));
        }
        ParamKind::Curve(s) => {
            m.insert("kind".into(), Value::from("curve"));
            m.insert("unit".into(), Value::from(s.unit.as_str()));
            m.insert("min".into(), num(s.min));
            m.insert("max".into(), num(s.max));
            m.insert(
                "default".into(),
                Value::Array(
                    s.default
                        .iter()
                        .map(|b| json!([num(b.frequency_hz), num(b.value)]))
                        .collect(),
                ),
            );
        }
    }
    Value::Object(m)
}

fn check_json(e: &CatalogEntry) -> Value {
    let hardware = may_arm_on_hardware(e);
    let (status, record) = match &e.check {
        CheckStatus::BuiltIn => ("built_in", None),
        CheckStatus::Unchecked => ("unchecked", None),
        CheckStatus::Unreadable(_) => ("unreadable", None),
        CheckStatus::Checked(r) if r.passed => ("passed", Some(r)),
        CheckStatus::Checked(r) => ("failed", Some(r)),
    };
    let message = match &hardware {
        Ok(()) if e.builtin => "Built in: cleared for hardware.".to_owned(),
        Ok(()) => "Checked against the simulated test articles and passed: cleared for hardware."
            .to_owned(),
        Err(why) => {
            let why = why.to_string();
            let mut c = why.chars();
            match c.next() {
                Some(f) => format!("{}{}.", f.to_uppercase(), c.as_str()),
                None => "Not cleared for hardware.".to_owned(),
            }
        }
    };
    json!({
        "status": status,
        "date": record.map(|r| r.date.clone()),
        "tool_version": record.map(|r| r.tool_version.clone()),
        "cases": record.map_or_else(Vec::new, |r| r.cases.iter().map(|c| json!({
            "name": c.name, "passed": c.passed, "detail": c.detail,
        })).collect()),
        "hardware": hardware.is_ok(),
        "message": message,
    })
}

/// One catalog entry on the wire. A Sine entry alone carries `live`.
pub fn entry_json(e: &CatalogEntry) -> Value {
    let mut v = entry_json_common(e);
    if e.world == LawWorld::Sine {
        if let Value::Object(m) = &mut v {
            m.insert("live".into(), Value::from(e.live.clone()));
        }
    }
    v
}

fn entry_json_common(e: &CatalogEntry) -> Value {
    let layout = |key: &str| e.mimo.as_ref().and_then(|m| m.layout(key)).map(layout_name);
    json!({
        "id": e.id,
        "world": world_name(e.world),
        "builtin": e.builtin,
        "name": e.identity.name,
        "version": e.identity.version,
        "author": e.identity.author,
        "summary": e.manifest.summary,
        "sha256": if e.identity.sha256.is_empty() { Value::Null } else { Value::from(e.identity.sha256.as_str()) },
        "check": check_json(e),
        "params": e.manifest.params.iter().map(param_json).collect::<Vec<_>>(),
        "diagnostics": e.manifest.diagnostics.iter().map(|d| json!({
            "key": d.key, "label": d.label, "unit": d.unit, "help": d.help,
            "layout": layout(&d.key),
        })).collect::<Vec<_>>(),
        "shapes": e.mimo.as_ref().map(|m| m.shapes.iter().map(|s| shape_name(*s)).collect::<Vec<_>>()),
        "needs": e.mimo.as_ref().map(|m| m.needs.iter().map(|i| input_name(*i)).collect::<Vec<_>>()),
    })
}

/// The `laws.list` result for `world` (`None`: every world, single-output Random
/// first, then multiple-output Random, then classical shock, then SRS shock, then
/// Sine, then TWR).
pub fn list_json(catalog: &LawCatalog, world: Option<LawWorld>) -> Value {
    let mut laws = Vec::new();
    for w in [
        LawWorld::Single,
        LawWorld::Mimo,
        LawWorld::ShockClassical,
        LawWorld::ShockSrs,
        LawWorld::Sine,
        LawWorld::Twr,
    ] {
        if world.is_none_or(|x| x == w) {
            laws.extend(catalog.entries_for(w).iter().map(entry_json));
        }
    }
    json!({
        "laws": laws,
        "problems": catalog.problems().iter().map(|p| json!({
            "file": p.path.file_name().and_then(|n| n.to_str()).unwrap_or(""),
            "reason": p.reason,
        })).collect::<Vec<_>>(),
    })
}

/// `laws.list`'s `world` parameter.
pub fn parse_world(params: &Map<String, Value>) -> Result<Option<LawWorld>, MethodError> {
    match crate::protocol::named_str(params, "world")? {
        None => Ok(None),
        Some("single") => Ok(Some(LawWorld::Single)),
        Some("mimo") => Ok(Some(LawWorld::Mimo)),
        Some("shock_classical") => Ok(Some(LawWorld::ShockClassical)),
        Some("shock_srs") => Ok(Some(LawWorld::ShockSrs)),
        Some("sine") => Ok(Some(LawWorld::Sine)),
        Some("twr") => Ok(Some(LawWorld::Twr)),
        Some(_) => Err(MethodError::bad_params(
            "world",
            "\"world\" is \"single\", \"mimo\", \"shock_classical\", \"shock_srs\", \
             \"sine\" or \"twr\", or absent for every world",
        )),
    }
}

/// The refusal when the catalog cannot be opened.
pub fn catalog_unavailable(e: &CatalogError) -> MethodError {
    MethodError::named(
        ErrorCode::Refused,
        "catalog_unavailable",
        format!("The control-law catalog could not be read ({e}). Check that the data folder is readable, then try again."),
    )
}

/// Import the law file at `path` into `catalog`.
pub fn import(catalog: &mut LawCatalog, path: &str) -> Result<Value, MethodError> {
    if path.trim().is_empty() {
        return Err(MethodError::bad_params(
            "path",
            "Importing a law needs the path of its file.",
        ));
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        return Err(MethodError::bad_params(
            "path",
            "Give the law file's full path on this computer.",
        ));
    }
    catalog
        .import(p)
        .map(|e| json!({"law": entry_json(&e)}))
        .map_err(|e| {
            let message = match &e {
                CatalogError::NotALaw { reason, .. } => format!(
                    "{} is not a control law this program can run ({reason}). Choose a law file built for it.",
                    p.display()
                ),
                CatalogError::Io { reason, .. } => format!(
                    "{} could not be read ({reason}). Check the path and that the file is readable.",
                    p.display()
                ),
                other => format!("The law was not imported ({other}). Try again."),
            };
            MethodError::named(ErrorCode::Refused, "law_import_failed", message)
                .with_detail(json!({"path": path}))
        })
}

/// A catalog entry as the MIMO setup's checks see it.
pub fn law_info(e: &CatalogEntry) -> crate::mimo::setup::LawInfo {
    crate::mimo::setup::LawInfo {
        id: e.id.clone(),
        name: e.identity.name.clone(),
        version: e.identity.version.clone(),
        sha256: e.identity.sha256.clone(),
        mimo: e.mimo.clone(),
    }
}

/// The built-in classical law, `fd_correct`, as a classical configuration's checks
/// see it, from the catalog's own manifest of it (for a check with no catalog at
/// hand, as `shock.preview`'s).
pub fn builtin_classical_facts() -> crate::shock::config::LawFacts {
    let m = hertzforge_random_laws::classical_builtin_manifest();
    crate::shock::config::LawFacts {
        id: hertzforge_random_laws::CLASSICAL_BUILTIN_ID.to_owned(),
        name: m.name,
        version: m.version,
        sha256: String::new(),
        builtin: true,
        world: Ok(()),
        params: m.params,
    }
}

/// A catalog entry as a classical configuration's checks see it: a Random law is
/// refused by the test type it is for.
pub fn classical_facts(e: &CatalogEntry) -> crate::shock::config::LawFacts {
    crate::shock::config::LawFacts {
        id: e.id.clone(),
        name: e.identity.name.clone(),
        version: e.identity.version.clone(),
        sha256: e.identity.sha256.clone(),
        builtin: e.builtin,
        world: match e.world {
            LawWorld::Single => Err("single-output Random"),
            LawWorld::Mimo => Err("multiple-output Random"),
            LawWorld::ShockClassical => Ok(()),
            LawWorld::ShockSrs => Err("SRS shock"),
            LawWorld::Sine => Err("Sine"),
            LawWorld::Twr => Err("TWR"),
        },
        params: e.manifest.params.clone(),
    }
}

/// The law a run names itself by.
pub fn law_ref(i: &crate::mimo::setup::LawInfo) -> crate::mimo::driver::LawRef {
    crate::mimo::driver::LawRef {
        id: i.id.clone(),
        name: i.name.clone(),
        version: i.version.clone(),
        sha256: i.sha256.clone(),
    }
}

/// The built-in SRS law, `srs_ratio`, as an SRS configuration's checks see it,
/// from the catalog's own manifest of it (for a check with no catalog at hand, as
/// `srs.preview`'s).
pub fn builtin_srs_facts() -> crate::shock::config::LawFacts {
    let m = hertzforge_random_laws::srs_builtin_manifest();
    crate::shock::config::LawFacts {
        id: hertzforge_random_laws::SRS_BUILTIN_ID.to_owned(),
        name: m.name,
        version: m.version,
        sha256: String::new(),
        builtin: true,
        world: Ok(()),
        params: m.params,
    }
}

/// A catalog entry as an SRS configuration's checks see it: a law of another
/// world is refused by the test type it is for.
pub fn srs_facts(e: &CatalogEntry) -> crate::shock::config::LawFacts {
    crate::shock::config::LawFacts {
        id: e.id.clone(),
        name: e.identity.name.clone(),
        version: e.identity.version.clone(),
        sha256: e.identity.sha256.clone(),
        builtin: e.builtin,
        world: match e.world {
            LawWorld::Single => Err("single-output Random"),
            LawWorld::Mimo => Err("multiple-output Random"),
            LawWorld::ShockClassical => Err("classical shock"),
            LawWorld::ShockSrs => Ok(()),
            LawWorld::Sine => Err("Sine"),
            LawWorld::Twr => Err("TWR"),
        },
        params: e.manifest.params.clone(),
    }
}

/// The built-in Sine law's id (the catalog's).
pub use hertzforge_random_laws::SINE_BUILTIN_ID;

/// The built-in Sine law's manifest, from the core's declaration (the catalog's).
pub use hertzforge_random_laws::sine_builtin_manifest;

/// The built-in Sine law's catalog entry, for a check made with no catalog at hand: the
/// catalog's own declaration and live parameters.
pub fn builtin_sine_entry() -> CatalogEntry {
    let m = sine_builtin_manifest();
    CatalogEntry {
        id: SINE_BUILTIN_ID.to_owned(),
        identity: hertzforge_law_host::LawIdentity {
            name: m.name.clone(),
            version: m.version.clone(),
            author: m.author.clone(),
            sha256: String::new(),
        },
        manifest: m,
        builtin: true,
        check: CheckStatus::BuiltIn,
        world: LawWorld::Sine,
        mimo: None,
        live: hertzforge_random_laws::sine_builtin_live(),
    }
}

/// A catalog entry as a Sine configuration's checks see it: a law of another
/// world is refused by the test type it is for.
pub fn sine_facts(e: &CatalogEntry) -> crate::sine::config::LawFacts {
    crate::sine::config::LawFacts {
        id: e.id.clone(),
        name: e.identity.name.clone(),
        version: e.identity.version.clone(),
        sha256: e.identity.sha256.clone(),
        builtin: e.builtin,
        world: match e.world {
            LawWorld::Single => Err("single-output Random"),
            LawWorld::Mimo => Err("multiple-output Random"),
            LawWorld::ShockClassical => Err("classical shock"),
            LawWorld::ShockSrs => Err("SRS shock"),
            LawWorld::Sine => Ok(()),
            LawWorld::Twr => Err("TWR"),
        },
        params: e.manifest.params.clone(),
        live: e.live.clone(),
    }
}

/// The built-in Sine law as a Sine configuration's checks see it.
pub fn builtin_sine_facts() -> crate::sine::config::LawFacts {
    sine_facts(&builtin_sine_entry())
}

/// Look a Sine law up by id: the catalog's entry, or the built-in.
pub fn sine_lookup(
    catalog: Option<&LawCatalog>,
    id: &str,
) -> Option<crate::sine::config::LawFacts> {
    if let Some(e) = catalog.and_then(|c| c.entry(id)) {
        return Some(sine_facts(&e));
    }
    (id == SINE_BUILTIN_ID).then(builtin_sine_facts)
}

/// A catalog entry as a TWR configuration's checks see it: a law of another world
/// is refused by the test type it is for.
pub fn twr_facts(e: &CatalogEntry) -> crate::shock::config::LawFacts {
    crate::shock::config::LawFacts {
        id: e.id.clone(),
        name: e.identity.name.clone(),
        version: e.identity.version.clone(),
        sha256: e.identity.sha256.clone(),
        builtin: e.builtin,
        world: match e.world {
            LawWorld::Single => Err("single-output Random"),
            LawWorld::Mimo => Err("multiple-output Random"),
            LawWorld::ShockClassical => Err("classical shock"),
            LawWorld::ShockSrs => Err("SRS shock"),
            LawWorld::Sine => Err("Sine"),
            LawWorld::Twr => Ok(()),
        },
        params: e.manifest.params.clone(),
    }
}

/// The built-in TWR law's id.
pub use hertzforge_twr_laws::DEFAULT_TWR_LAW_ID as TWR_BUILTIN_ID;

/// The built-in TWR law as a TWR configuration's checks see it when the catalog
/// cannot be opened: its identity and parameters from its own manifest (the
/// catalog lists the same entry first).
pub fn builtin_twr_facts() -> crate::shock::config::LawFacts {
    let m = hertzforge_random_laws::twr_builtin_manifest();
    crate::shock::config::LawFacts {
        id: TWR_BUILTIN_ID.to_owned(),
        name: m.name.clone(),
        version: m.version.clone(),
        sha256: String::new(),
        builtin: true,
        world: Ok(()),
        params: m.params.clone(),
    }
}

/// Look a TWR law up by id: the catalog's entry, or the built-in.
pub fn twr_lookup(
    catalog: Option<&LawCatalog>,
    id: &str,
) -> Option<crate::shock::config::LawFacts> {
    if let Some(e) = catalog.and_then(|c| c.entry(id)) {
        return Some(twr_facts(&e));
    }
    (id == TWR_BUILTIN_ID).then(builtin_twr_facts)
}

/// The test type a law of `world` is for, in words.
fn world_words(world: LawWorld) -> &'static str {
    match world {
        LawWorld::Single => "single-output Random",
        LawWorld::Mimo => "multiple-output Random",
        LawWorld::ShockClassical => "classical shock",
        LawWorld::ShockSrs => "SRS shock",
        LawWorld::Sine => "Sine",
        LawWorld::Twr => "TWR",
    }
}

/// Whether a law may drive hardware in a Mixed Mode role (design D15): a
/// built-in may; an imported law needs a passed check in its own world (a
/// `single` law has none to need) AND a passed mixed check in this role.
/// `mixed_passed` is that role's mixed-check record for the exact file.
pub fn mixed_may_arm_on_hardware(e: &CatalogEntry, mixed_passed: bool) -> bool {
    if e.builtin {
        return true;
    }
    let own_world = match e.world {
        LawWorld::Single => true,
        _ => may_arm_on_hardware(e).is_ok(),
    };
    own_world && mixed_passed
}

/// A catalog entry as a Mixed Mode role sees it: a law of another world than
/// the role's is refused by the test type it is for.
pub fn mixed_role_facts(
    e: &CatalogEntry,
    want: LawWorld,
    mixed_passed: bool,
) -> crate::mixed::config::RoleLaw {
    crate::mixed::config::RoleLaw {
        id: e.id.clone(),
        name: e.identity.name.clone(),
        version: e.identity.version.clone(),
        sha256: e.identity.sha256.clone(),
        builtin: e.builtin,
        world: if e.world == want {
            Ok(())
        } else {
            Err(world_words(e.world))
        },
        params: e.manifest.params.clone(),
        live: if want == LawWorld::Sine {
            e.live.clone()
        } else {
            Vec::new()
        },
        hardware: mixed_may_arm_on_hardware(e, mixed_passed),
    }
}

/// Look a Mixed Mode role's law up by id. No mixed-check record is kept yet, so
/// an imported law is never cleared for hardware in a Mixed Mode role.
pub fn mixed_lookup(
    catalog: Option<&LawCatalog>,
    id: &str,
    want: LawWorld,
) -> Option<crate::mixed::config::RoleLaw> {
    if let Some(e) = catalog.and_then(|c| c.entry(id)) {
        return Some(mixed_role_facts(&e, want, false));
    }
    (want == LawWorld::Sine && id == SINE_BUILTIN_ID)
        .then(|| mixed_role_facts(&builtin_sine_entry(), want, false))
}
