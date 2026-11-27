//! The MIMO Sine law choice: the one place a configuration's law
//! (`control.law`) becomes the factory a run builds its law with, built-in or
//! law file, and the `sine_mimo` world's entries for `laws.list`, each with
//! the shapes, test types and references it accepts and the default marked.

use crate::protocol::{ErrorCode, MethodError};
use hertzforge_law_host::{DiagnosticSpec, ParamSpec};
use hertzforge_mimo_sine::{shape_name, MimoShape, MimoSineLawFactory, TestType, DEFAULT_LAW_ID};
use hertzforge_mimo_sine_laws as msl;
use hertzforge_random_laws::{may_arm_on_hardware, LawCatalog, LawWorld};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::Duration;

/// What a configuration's checks need to know about a law named by id.
#[derive(Debug, Clone, PartialEq)]
pub struct LawFacts {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Empty for a built-in law.
    pub sha256: String,
    pub builtin: bool,
    /// `Err` with the test type it is for when it is not a MIMO Sine law.
    pub world: Result<(), &'static str>,
    pub params: Vec<ParamSpec>,
    pub shapes: Vec<MimoShape>,
    pub test_types: Vec<TestType>,
    pub amplitude_only: bool,
    /// The values it reports, labelled.
    pub values: Vec<DiagnosticSpec>,
}

impl LawFacts {
    /// The law as `derived` names it: identity and what it accepts.
    pub fn derived_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "version": self.version,
            "sha256": if self.sha256.is_empty() { Value::Null } else { Value::from(self.sha256.as_str()) },
            "shapes": self.shapes.iter().map(|s| shape_name(*s)).collect::<Vec<_>>(),
            "test_types": self.test_types.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
            "amplitude_only": self.amplitude_only,
        })
    }
}

/// The facts a configuration's checks see for `id`, or `None` when no law has
/// that id.
pub fn facts(cat: &LawCatalog, id: &str) -> Option<LawFacts> {
    let entries = msl::entries(cat);
    if let Some(f) = msl::all_facts(cat).into_iter().find(|f| f.id == id) {
        let values = entries
            .iter()
            .find(|e| e.entry.id == id)
            .map(|e| e.entry.manifest.diagnostics.clone())
            .unwrap_or_default();
        return Some(LawFacts {
            id: f.id,
            name: f.identity.name,
            version: f.identity.version,
            sha256: f.identity.sha256,
            builtin: f.builtin,
            world: Ok(()),
            params: f.params,
            shapes: f.shapes,
            test_types: f.test_types,
            amplitude_only: f.amplitude_only,
            values,
        });
    }
    let e = cat.entry(id)?;
    Some(LawFacts {
        id: e.id.clone(),
        name: e.identity.name.clone(),
        version: e.identity.version.clone(),
        sha256: e.identity.sha256.clone(),
        builtin: e.builtin,
        world: Err(if e.world == LawWorld::SineMimo {
            "MIMO Sine"
        } else {
            e.world.test_type()
        }),
        params: e.manifest.params.clone(),
        shapes: Vec::new(),
        test_types: Vec::new(),
        amplitude_only: false,
        values: Vec::new(),
    })
}

/// The built-in laws that run on the simulator only: each fails a scored case
/// of the law check (`followed_poles` over-tests the steep reference), so a
/// hardware station refuses it as it refuses a law file without a passed
/// check.
pub const SIMULATOR_ONLY: [&str; 1] = ["followed_poles"];

/// Resolve the law `id` with `given` parameter values for a run: the one seam
/// a law choice passes through. On `hardware`, a law file arms only with a
/// passed check for its exact file, and a built-in law of [`SIMULATOR_ONLY`]
/// not at all; the simulator takes any law.
pub fn resolve(
    cat: &LawCatalog,
    id: &str,
    given: &Map<String, Value>,
    budget: Duration,
    build_budget: Duration,
    hardware: bool,
) -> Result<Arc<dyn MimoSineLawFactory>, MethodError> {
    let f = facts(cat, id).ok_or_else(|| {
        MethodError::named(
            ErrorCode::NotFound,
            "law_unknown",
            format!("No control law \"{id}\" is in the catalog."),
        )
    })?;
    if hardware && f.builtin && SIMULATOR_ONLY.contains(&id) {
        return Err(MethodError::named(
            ErrorCode::Refused,
            "law_not_checked",
            format!(
                "The control law \"{}\" does not pass the law check; it runs on the simulator \
                 only.",
                f.name
            ),
        ));
    }
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
    msl::resolve_for_run(cat, id, params, budget, build_budget).map_err(|e| {
        MethodError::named(
            ErrorCode::Refused,
            "arm_refused",
            format!("The control law could not be built: {e}."),
        )
    })
}

/// The `sine_mimo` world's entries for `laws.list`: the built-ins first
/// (`matrix`, the default, then the rest), then the imported files, each with
/// the shapes, test types and references it accepts, `default` true on the
/// world's default law alone, and a built-in law's stated `limits`.
pub fn list(cat: &LawCatalog) -> Vec<Value> {
    msl::entries(cat)
        .iter()
        .map(|e| {
            let mut v = crate::laws::entry_json(&e.entry);
            if let Value::Object(m) = &mut v {
                m.insert("shapes".into(), Value::from(e.shape_names()));
                m.insert("test_types".into(), Value::from(e.test_type_names()));
                m.insert("amplitude_only".into(), Value::from(e.amplitude_only));
                m.insert(
                    "default".into(),
                    Value::from(e.entry.builtin && e.entry.id == DEFAULT_LAW_ID),
                );
                m.insert(
                    "limits".into(),
                    if e.entry.builtin {
                        msl::limits(&e.entry.id).map_or(Value::Null, Value::from)
                    } else {
                        Value::Null
                    },
                );
                if e.entry.builtin && SIMULATOR_ONLY.contains(&e.entry.id.as_str()) {
                    m.insert(
                        "check".into(),
                        json!({
                            "status": "built_in", "date": null, "tool_version": null,
                            "cases": [], "hardware": false,
                            "message": "Built in, on the simulator only: it does not pass the \
                                        law check.",
                        }),
                    );
                }
            }
            v
        })
        .collect()
}

/// The one built-in law's entry for `id`, for a check's answer.
pub fn entry_json(cat: &LawCatalog, id: &str) -> Option<Value> {
    list(cat).into_iter().find(|v| v["id"] == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_sine_the_list_marks_one_default_and_states_what_each_law_accepts() {
        let dir = std::env::temp_dir().join(format!("msine-laws-{}", std::process::id()));
        let cat = LawCatalog::open(&dir).expect("a catalog");
        let laws = list(&cat);
        let ids: Vec<&str> = laws.iter().filter_map(|l| l["id"].as_str()).collect();
        assert_eq!(
            ids,
            [
                "matrix",
                "paired",
                "steepest_descent",
                "secant",
                "ratio",
                "followed_poles"
            ]
        );
        let defaults: Vec<&str> = laws
            .iter()
            .filter(|l| l["default"] == true)
            .filter_map(|l| l["id"].as_str())
            .collect();
        assert_eq!(defaults, ["matrix"]);
        let hardware: Vec<&str> = laws
            .iter()
            .filter(|l| l["check"]["hardware"] == true)
            .filter_map(|l| l["id"].as_str())
            .collect();
        assert_eq!(
            hardware,
            ["matrix", "paired", "steepest_descent", "secant", "ratio"]
        );
        let of = |id: &str| laws.iter().find(|l| l["id"] == id).expect("listed").clone();
        assert_eq!(
            of("matrix")["test_types"],
            json!(["sweep", "dwell", "stepped"])
        );
        assert_eq!(of("ratio")["test_types"], json!(["dwell", "stepped"]));
        assert_eq!(of("secant")["test_types"], json!(["dwell"]));
        assert_eq!(of("secant")["amplitude_only"], false);
        assert_eq!(of("steepest_descent")["amplitude_only"], false);
        assert_eq!(of("matrix")["amplitude_only"], true);
        assert!(of("matrix")["limits"].as_str().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
