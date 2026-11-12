//! The MIMO Shock law choice: the one place a configuration's law (`control.law`)
//! becomes the factory a run builds its law with, built-in or law file, and the
//! `shock_mimo` world's entries for `laws.list`.

use super::config::LawFacts;
use crate::protocol::{ErrorCode, MethodError};
use hertzforge_law_host::DiagnosticSpec;
use hertzforge_mimo_shock::{MimoShockLawFactory, TargetKind};
use hertzforge_mimo_shock_laws::{self as msl, HostRule};
use hertzforge_random_laws::{may_arm_on_hardware, LawCatalog, LawWorld};
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Duration;

/// A law chosen for a run.
pub struct Chosen {
    pub factory: Arc<dyn MimoShockLawFactory>,
    /// For a law file: the host's name for the last output it refused.
    pub host_rule: Option<HostRule>,
    /// The values it reports, labelled.
    pub values: Vec<DiagnosticSpec>,
}

fn kind_name(k: TargetKind) -> &'static str {
    k.as_str()
}

/// The facts a configuration's checks see for `id`, or `None` when no law has
/// that id.
pub fn facts(cat: &LawCatalog, id: &str) -> Option<LawFacts> {
    if let Some(f) = msl::all_facts(cat).into_iter().find(|f| f.id == id) {
        return Some(LawFacts {
            id: f.id,
            name: f.identity.name,
            version: f.identity.version,
            sha256: f.identity.sha256,
            builtin: f.builtin,
            world: Ok(()),
            targets: f.targets,
            params: f.params,
        });
    }
    let e = cat.entry(id)?;
    Some(LawFacts {
        id: e.id.clone(),
        name: e.identity.name.clone(),
        version: e.identity.version.clone(),
        sha256: e.identity.sha256.clone(),
        builtin: e.builtin,
        world: Err(if e.world == LawWorld::ShockMimo {
            "MIMO Shock"
        } else {
            e.world.test_type()
        }),
        targets: Vec::new(),
        params: e.manifest.params.clone(),
    })
}

/// Resolve the law `id` with `given` parameter values for a run: the one seam a
/// law choice passes through. On `hardware`, a law file arms only with a passed
/// check for its exact file; the simulator takes any law.
pub fn resolve(
    cat: &LawCatalog,
    id: &str,
    given: &Map<String, Value>,
    budget: Duration,
    seed_budget: Duration,
    hardware: bool,
) -> Result<Chosen, MethodError> {
    let f = facts(cat, id).ok_or_else(|| {
        MethodError::named(
            ErrorCode::NotFound,
            "law_unknown",
            format!("No control law \"{id}\" is in the catalog."),
        )
    })?;
    if hardware && !f.builtin {
        if let Some(entry) = cat.entry(id) {
            if may_arm_on_hardware(&entry).is_err() {
                return Err(MethodError::named(
                    ErrorCode::Refused,
                    "law_not_checked",
                    format!(
                        "The control law \"{}\" has no passed check for this file; run \
                         laws.check on it before it drives hardware.",
                        f.name
                    ),
                ));
            }
        }
    }
    let params = crate::mimo::adapter::params_of(&f.params, given);
    let (factory, host_rule) =
        msl::resolve_for_run(cat, id, params, budget, seed_budget).map_err(|e| {
            MethodError::named(
                ErrorCode::Refused,
                "arm_refused",
                format!("The control law could not be built: {e}."),
            )
        })?;
    let values = msl::entries(cat)
        .into_iter()
        .find(|e| e.id == id)
        .map(|e| e.manifest.diagnostics)
        .unwrap_or_default();
    Ok(Chosen {
        factory,
        host_rule,
        values,
    })
}

/// The `shock_mimo` world's entries for `laws.list`: the built-ins first, then
/// the imported files, each with the target kinds it accepts.
pub fn list(cat: &LawCatalog) -> Vec<Value> {
    let facts = msl::all_facts(cat);
    msl::entries(cat)
        .iter()
        .map(|e| {
            let mut v = crate::laws::entry_json(e);
            let targets: Vec<&str> = facts
                .iter()
                .find(|f| f.id == e.id)
                .map(|f| f.targets.iter().map(|k| kind_name(*k)).collect())
                .unwrap_or_default();
            // The target kinds this law is the default for (a built-in only):
            // what a client opens a new setup of that kind on.
            let default_for: Vec<&str> = msl::BuiltinMimoShock::from_id(&e.id)
                .filter(|_| e.builtin)
                .map(|b| b.default_kinds().into_iter().map(kind_name).collect())
                .unwrap_or_default();
            if let Value::Object(m) = &mut v {
                m.insert("targets".into(), Value::from(targets));
                m.insert("default_for".into(), Value::from(default_for));
            }
            v
        })
        .collect()
}
