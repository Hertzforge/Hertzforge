//! The control-law catalog on the wire (schema/PROTOCOL.md, "Control Laws"):
//! `laws.list` and `laws.import`, for single-output and multiple-output Random
//! alike. The catalog is the controller's own (`hertzforge-random-laws`): the
//! built-in laws, and the law files imported into the local data root.

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

/// One catalog entry on the wire.
pub fn entry_json(e: &CatalogEntry) -> Value {
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

/// The `laws.list` result for `world` (`None`: both worlds, single-output first).
pub fn list_json(catalog: &LawCatalog, world: Option<LawWorld>) -> Value {
    let mut laws = Vec::new();
    if world != Some(LawWorld::Mimo) {
        laws.extend(catalog.entries().iter().map(entry_json));
    }
    if world != Some(LawWorld::Single) {
        laws.extend(catalog.mimo_entries().iter().map(entry_json));
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
        Some(_) => Err(MethodError::bad_params(
            "world",
            "\"world\" is \"single\" or \"mimo\", or absent for both",
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

/// The law a run names itself by.
pub fn law_ref(i: &crate::mimo::setup::LawInfo) -> crate::mimo::driver::LawRef {
    crate::mimo::driver::LawRef {
        id: i.id.clone(),
        name: i.name.clone(),
        version: i.version.clone(),
        sha256: i.sha256.clone(),
    }
}
