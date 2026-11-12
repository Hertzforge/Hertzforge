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
pub use wasm::{shock_mimo_law_fault, WasmMimoShockLaw, WasmMimoShockLawFactory};

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
        }
    }
}

impl std::error::Error for ResolveError {}

/// The factory for the built-in law `id`.
pub fn resolve_builtin(id: &str) -> Result<Arc<dyn MimoShockLawFactory>, ResolveError> {
    match BuiltinMimoShock::from_id(id) {
        Some(b) => Err(ResolveError::NotBuilt(b.id())),
        None => Err(ResolveError::Unknown(id.into())),
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
        return resolve_builtin(id);
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
    fn mimo_shock_resolving_a_law_fails_loudly_until_it_is_built() {
        assert_eq!(
            resolve_builtin("inverse").err(),
            Some(ResolveError::NotBuilt("inverse"))
        );
        assert_eq!(
            resolve_builtin("nope").err(),
            Some(ResolveError::Unknown("nope".into()))
        );
    }
}
