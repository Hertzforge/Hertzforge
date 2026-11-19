//! The choice of a MIMO Shock law: the built-in laws by name, law files by
//! catalog id or by path, the facts the law picker shows, the law check's cases
//! and their requirements, and a side-by-side run that shows two laws give the
//! same drives.
//!
//! - [`resolve`]: a catalog id to the factory a run builds its law with. A
//!   built-in id goes to [`resolve_builtin`]; any other is a `shock-mimo` law
//!   file in the catalog, run through the law host ([`wasm`]).
//! - [`load_law_file`]: a law file by path, as a contributor tries one.
//! - [`facts`]: the catalog entries and the picker's facts, built-ins first.
//! - [`check`]: the cases, their requirements and the judge.
//! - [`parity`]: two laws over one scripted run.

pub mod check;
pub mod facts;
pub mod parity;
pub mod wasm;

pub use check::{
    check_record, judge, run_check, CaseRun, CaseSpec, CaseVerdict, MimoShockCheckCase,
    Perturbation, MIMO_SHOCK_CHECK_VERSION,
};
pub use facts::{all_facts, builtin_facts, entries, file_facts, KnownFailure, LawFacts};
pub use parity::{compare_laws, MathLaw, ParityReport};
pub use wasm::{shock_mimo_law_fault, HostRule, WasmMimoShockLaw, WasmMimoShockLawFactory};

use hertzforge_law_host::{LawHost, LoadedShockMimoLaw, Param};
use hertzforge_mimo_shock::{MimoShockLawFactory, TargetKind};
use hertzforge_random_laws::{LawCatalog, LawWorld};
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// A built-in MIMO Shock law.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinMimoShock {
    /// The regularized inverse of the plant per line, corrected between pulses.
    Inverse,
    /// Magnitude correction of each control's target from its SRS ratio.
    Shape,
    /// Each control paired with one drive, scaled by its own magnitude ratio.
    Paired,
}

impl BuiltinMimoShock {
    /// Every built-in law, the default for each target kind first.
    pub const ALL: [BuiltinMimoShock; 3] = [
        BuiltinMimoShock::Inverse,
        BuiltinMimoShock::Shape,
        BuiltinMimoShock::Paired,
    ];

    /// The target kinds this built-in is the default for.
    pub fn default_kinds(self) -> Vec<TargetKind> {
        TargetKind::ALL
            .into_iter()
            .filter(|k| Self::default_for(*k) == self)
            .collect()
    }

    /// The catalog id.
    pub fn id(self) -> &'static str {
        match self {
            BuiltinMimoShock::Inverse => "inverse",
            BuiltinMimoShock::Shape => "shape",
            BuiltinMimoShock::Paired => "paired",
        }
    }

    /// The built-in law with catalog id `id`.
    pub fn from_id(id: &str) -> Option<BuiltinMimoShock> {
        Self::ALL.into_iter().find(|b| b.id() == id)
    }

    /// The target kinds the law accepts.
    pub fn targets(self) -> &'static [TargetKind] {
        match self {
            BuiltinMimoShock::Inverse | BuiltinMimoShock::Paired => &TargetKind::ALL,
            BuiltinMimoShock::Shape => &[TargetKind::Srs],
        }
    }

    /// The default law for a target kind: `inverse` for a waveform, `shape` for
    /// an SRS.
    pub fn default_for(kind: TargetKind) -> BuiltinMimoShock {
        match kind {
            TargetKind::Waveform => BuiltinMimoShock::Inverse,
            TargetKind::Srs => BuiltinMimoShock::Shape,
        }
    }
}

/// Why a law could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    /// No built-in law has this id.
    Unknown(String),
    /// The law is named but this version does not run it.
    NotBuilt(&'static str),
    /// The id is a law file of another test type.
    WrongWorld {
        /// The id asked for.
        id: String,
        /// The test type the law file is for.
        law_is_for: &'static str,
    },
    /// The law file could not be read or loaded.
    File(String),
    /// A parameter the law does not declare, or a value it cannot take.
    BadParams(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Unknown(id) => write!(f, "no MIMO Shock law has the id `{id}`"),
            ResolveError::NotBuilt(id) => {
                write!(f, "the MIMO Shock law `{id}` cannot run in this version")
            }
            ResolveError::WrongWorld { id, law_is_for } => write!(
                f,
                "the law `{id}` is a {law_is_for} law, not a MIMO Shock law"
            ),
            ResolveError::File(why) => write!(f, "the MIMO Shock law file: {why}"),
            ResolveError::BadParams(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for ResolveError {}

/// The factory for the built-in law `id` at its declared defaults.
pub fn resolve_builtin(id: &str) -> Result<Arc<dyn MimoShockLawFactory>, ResolveError> {
    resolve_builtin_with(id, &[])
}

/// The factory for the built-in law `id` with `params` set over its declared
/// defaults. The values are read by the SDK's declarations of the built-ins
/// (`shock_mimo::builtin`), the same readers a law file of the law uses, and
/// handed to the engine's native law: one set of keys, ranges and defaults.
pub fn resolve_builtin_with(
    id: &str,
    params: &[Param],
) -> Result<Arc<dyn MimoShockLawFactory>, ResolveError> {
    use hertzforge_law_sdk::shock_mimo::builtin as decl;
    use hertzforge_mimo_shock::builtin::{InverseFactory, PairedFactory, ShapeFactory};
    let Some(b) = BuiltinMimoShock::from_id(id) else {
        return Err(ResolveError::Unknown(id.into()));
    };
    let d = facts::decl(b);
    let mut all = decl::defaults(d);
    for p in params {
        if !d.params.iter().any(|k| k.key == p.key) {
            return Err(ResolveError::BadParams(format!(
                "the law `{id}` has no parameter `{}`",
                p.key
            )));
        }
        let v = sdk_value(&p.value);
        match all.0.iter_mut().find(|q| q.key == p.key) {
            Some(q) => q.value = v,
            None => all.0.push(hertzforge_law_sdk::Param {
                key: p.key.clone(),
                value: v,
            }),
        }
    }
    let bad = |e: String| ResolveError::BadParams(format!("the law `{id}`: {e}"));
    Ok(match b {
        BuiltinMimoShock::Inverse => {
            let c = decl::inverse_config(&all).map_err(bad)?;
            Arc::new(InverseFactory {
                params: c.params,
                model: c.model,
                predicted_step: c.predicted_step,
            })
        }
        BuiltinMimoShock::Shape => {
            let c = decl::shape_config(&all).map_err(bad)?;
            Arc::new(ShapeFactory {
                params: c.params,
                model: c.model,
                gain_clamp_db: c.gain_clamp_db,
                stall_tol_db: c.stall_tol_db,
            })
        }
        BuiltinMimoShock::Paired => {
            let count = all.integer("smooth_lines").map_err(bad)?;
            Arc::new(PairedFactory {
                drive_of: None,
                smooth_lines: usize::try_from(count)
                    .map_err(|_| bad(format!("{count} smoothing lines")))?,
                weight: all.number("weight").map_err(bad)?,
                max_step_db: all.number("max_step_db").map_err(bad)?,
            })
        }
    })
}

/// A host parameter value as the SDK's.
fn sdk_value(v: &hertzforge_law_host::ParamValue) -> hertzforge_law_sdk::ParamValue {
    use hertzforge_law_host::ParamValue as H;
    use hertzforge_law_sdk::ParamValue as S;
    match v {
        H::Number(x) => S::Number(*x),
        H::Integer(x) => S::Integer(*x),
        H::Toggle(x) => S::Toggle(*x),
        H::Choice(x) => S::Choice(x.clone()),
        H::PerChannel(x) => S::PerChannel(x.clone()),
        H::Curve(x) => S::Curve(
            x.iter()
                .map(|b| hertzforge_law_sdk::Breakpoint {
                    frequency_hz: b.frequency_hz,
                    value: b.value,
                })
                .collect(),
        ),
    }
}

/// The factory a run builds its law with, for the law with catalog id `id`:
/// a built-in by name ([`resolve_builtin`]), or a `shock-mimo` law file in
/// `catalog` by its SHA-256, built with `params` and with `budget` as every
/// call's deadline.
pub fn resolve(
    catalog: &LawCatalog,
    id: &str,
    params: Vec<Param>,
    budget: Duration,
) -> Result<Arc<dyn MimoShockLawFactory>, ResolveError> {
    if BuiltinMimoShock::from_id(id).is_some() {
        return resolve_builtin_with(id, &params);
    }
    if let Some(law) = catalog.shock_mimo_law(id) {
        return Ok(Arc::new(WasmMimoShockLawFactory::new(
            law.clone(),
            params,
            budget,
        )));
    }
    match catalog.entry(id) {
        Some(e) if e.world != LawWorld::ShockMimo => Err(ResolveError::WrongWorld {
            id: id.into(),
            law_is_for: e.world.test_type(),
        }),
        _ => Err(ResolveError::Unknown(id.into())),
    }
}

/// [`resolve`] for a run: the factory, and for a law file the cell its host
/// fills with its own name for the last output it refused, so the run's end can
/// name it where the core's fault folds it into `wrong_length`. A law file's
/// constructor and seed each have `seed_budget`.
pub fn resolve_for_run(
    catalog: &LawCatalog,
    id: &str,
    params: Vec<Param>,
    budget: Duration,
    seed_budget: Duration,
) -> Result<(Arc<dyn MimoShockLawFactory>, Option<HostRule>), ResolveError> {
    if BuiltinMimoShock::from_id(id).is_none() {
        if let Some(law) = catalog.shock_mimo_law(id) {
            let f = WasmMimoShockLawFactory::new(law.clone(), params, budget)
                .with_seed_budget(seed_budget);
            let rule = f.host_rule();
            return Ok((Arc::new(f), Some(rule)));
        }
    }
    resolve(catalog, id, params, budget).map(|f| (f, None))
}

/// Load the MIMO Shock law file at `path` with `host`: how a contributor runs
/// a law straight from the build, before importing it into a catalog.
pub fn load_law_file(host: &LawHost, path: &Path) -> Result<LoadedShockMimoLaw, ResolveError> {
    let bytes =
        std::fs::read(path).map_err(|e| ResolveError::File(format!("{}: {e}", path.display())))?;
    host.load_shock_mimo(&bytes)
        .map_err(|e| ResolveError::File(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mimo_shock_builtin_ids_are_plain_and_round_trip() {
        let ids: Vec<&str> = BuiltinMimoShock::ALL.iter().map(|b| b.id()).collect();
        assert_eq!(ids, ["inverse", "shape", "paired"]);
        for b in BuiltinMimoShock::ALL {
            assert_eq!(BuiltinMimoShock::from_id(b.id()), Some(b));
            assert!(b.targets().contains(&TargetKind::Srs));
        }
        assert_eq!(
            BuiltinMimoShock::default_for(TargetKind::Waveform),
            BuiltinMimoShock::Inverse
        );
        assert_eq!(
            BuiltinMimoShock::default_for(TargetKind::Srs),
            BuiltinMimoShock::Shape
        );
        assert!(!BuiltinMimoShock::Shape
            .targets()
            .contains(&TargetKind::Waveform));
    }

    #[test]
    fn mimo_shock_a_builtin_resolves_to_the_engine_law_by_name() {
        for b in BuiltinMimoShock::ALL {
            let f = resolve_builtin(b.id()).unwrap();
            assert_eq!(f.describe().id, b.id());
            assert_eq!(
                f.manifest().targets,
                b.targets().to_vec(),
                "{}: the engine law accepts what the declaration says",
                b.id()
            );
        }
        assert_eq!(
            resolve_builtin("nope").err(),
            Some(ResolveError::Unknown("nope".into()))
        );
    }

    #[test]
    fn mimo_shock_the_declared_defaults_are_the_engine_laws_defaults() {
        use hertzforge_mimo_shock::builtin::{InverseFactory, PairedFactory, ShapeFactory};
        let d = |id: &str| resolve_builtin(id).unwrap().describe().params;
        assert_eq!(d("inverse"), InverseFactory::default().describe().params);
        assert_eq!(d("shape"), ShapeFactory::default().describe().params);
        assert_eq!(d("paired"), PairedFactory::default().describe().params);
    }

    #[test]
    fn mimo_shock_a_set_parameter_reaches_the_engine_law_and_a_stray_one_is_refused() {
        use hertzforge_law_host::ParamValue;
        let f = resolve_builtin_with(
            "inverse",
            &[Param {
                key: "gain".into(),
                value: ParamValue::Number(0.5),
            }],
        )
        .unwrap();
        assert_eq!(f.describe().params["gain"], 0.5);
        let f = resolve_builtin_with(
            "shape",
            &[Param {
                key: "delay_bound_ms".into(),
                value: ParamValue::Number(3.0),
            }],
        )
        .unwrap();
        assert_eq!(f.describe().params["delay_bound_s"], 0.003);
        let e = resolve_builtin_with(
            "paired",
            &[Param {
                key: "gain".into(),
                value: ParamValue::Number(0.5),
            }],
        )
        .err()
        .unwrap();
        assert!(matches!(e, ResolveError::BadParams(_)), "{e:?}");
    }
}
