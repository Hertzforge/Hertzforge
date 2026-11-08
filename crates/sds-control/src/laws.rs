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
            m.insert("live".into(), Value::from(sine_live_keys(e)));
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
/// Sine).
pub fn list_json(catalog: &LawCatalog, world: Option<LawWorld>) -> Value {
    let mut laws = Vec::new();
    for w in [
        LawWorld::Single,
        LawWorld::Mimo,
        LawWorld::ShockClassical,
        LawWorld::ShockSrs,
        LawWorld::Sine,
    ] {
        if world.is_none_or(|x| x == w) {
            let entries = sine_entries(catalog, w);
            laws.extend(entries.iter().map(entry_json));
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
        Some(_) => Err(MethodError::bad_params(
            "world",
            "\"world\" is \"single\", \"mimo\", \"shock_classical\", \"shock_srs\" or \
             \"sine\", or absent for every world",
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
        },
        params: e.manifest.params.clone(),
    }
}

/// A world's entries as `laws.list` gives them: the catalog's, with the built-in
/// Sine law first when the catalog does not list it yet.
fn sine_entries(catalog: &LawCatalog, w: LawWorld) -> Vec<CatalogEntry> {
    let mut entries = catalog.entries_for(w);
    if w == LawWorld::Sine && !entries.iter().any(|e| e.id == SINE_BUILTIN_ID) {
        entries.insert(0, builtin_sine_entry());
    }
    entries
}

/// The built-in Sine law's id.
pub const SINE_BUILTIN_ID: &str = hertzforge_sine::law::SERVO_BLEND_ID;

/// The keys of a Sine law's live parameters.
pub fn sine_live_keys(e: &CatalogEntry) -> Vec<String> {
    if e.builtin && e.id == SINE_BUILTIN_ID {
        SINE_BUILTIN_LIVE.iter().map(|k| (*k).to_owned()).collect()
    } else {
        Vec::new()
    }
}

/// The built-in Sine law's live parameters.
pub const SINE_BUILTIN_LIVE: [&str; 1] = ["blend_w"];

/// The built-in Sine law's manifest, `servo_blend`: the amplitude servo and the
/// feedforward blend, as the run declares it.
pub fn sine_builtin_manifest() -> hertzforge_law_host::Manifest {
    use hertzforge_law_host::{ChoiceSpec, DiagnosticSpec, Manifest, NumberSpec};
    let number =
        |key: &str, label: &str, help: &str, unit: &str, min, max, step, default| ParamSpec {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ParamKind::Number(NumberSpec {
                unit: unit.into(),
                min,
                max,
                step,
                default,
            }),
        };
    let diag = |key: &str, label: &str, help: &str| DiagnosticSpec {
        key: key.into(),
        label: label.into(),
        unit: "dB".into(),
        help: help.into(),
    };
    Manifest {
        name: "Servo blend".into(),
        version: "0.1.0".into(),
        author: "Hertzforge contributors".into(),
        summary: "Moves the tone's amplitude each tick by a proportional-integral servo on the \
                  error in dB, blended with the identification's prediction of the drive at the \
                  present frequency. The controller caps every change and holds the drive under \
                  the limit channels and the drive ceiling."
            .into(),
        params: vec![
            number(
                "blend_w",
                "Prediction weight",
                "How much of each tick's change follows the identification's prediction of the \
                 drive (1) rather than the servo on the measured error (0). It can be changed \
                 while the test runs.",
                "",
                0.0,
                1.0,
                0.05,
                0.5,
            ),
            ParamSpec {
                key: "compression".into(),
                label: "Compression".into(),
                help: "How fast the servo corrects: slow (integral gain 2, at most 20 dB/s), \
                       fast (6, at most 60 dB/s), or custom with the two values below."
                    .into(),
                kind: ParamKind::Choice(ChoiceSpec {
                    options: vec!["slow".into(), "fast".into(), "custom".into()],
                    default: "slow".into(),
                }),
            },
            number(
                "ki",
                "Integral gain",
                "The servo's integral gain, with custom compression.",
                "",
                0.1,
                20.0,
                0.1,
                2.0,
            ),
            number(
                "max_rate_db_per_s",
                "Largest servo rate",
                "The fastest the servo moves the drive, with custom compression.",
                "dB/s",
                1.0,
                240.0,
                1.0,
                20.0,
            ),
            number(
                "kp",
                "Proportional gain",
                "The servo's proportional gain on the error in dB.",
                "",
                0.0,
                2.0,
                0.05,
                0.3,
            ),
        ],
        diagnostics: vec![
            diag(
                "servo_step_db",
                "Servo step",
                "The servo's part of the last tick's change.",
            ),
            diag(
                "prediction_step_db",
                "Prediction step",
                "The prediction's part of the last tick's change.",
            ),
            diag(
                "total_step_db",
                "Total step",
                "The change the law asked for on the last tick, before the controller's caps.",
            ),
        ],
    }
}

/// The built-in Sine law's catalog entry.
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
        },
        params: e.manifest.params.clone(),
        live: if e.world == LawWorld::Sine {
            sine_live_keys(e)
        } else {
            Vec::new()
        },
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
