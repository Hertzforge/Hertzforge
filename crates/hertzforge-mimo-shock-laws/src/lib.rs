//! The choice of a MIMO Shock law: the built-in laws by name, resolution of a law
//! by its catalog id, and the law check's cases.
//!
//! The built-in laws (`inverse`, `shape`, `paired`) are named here so the catalog
//! and the wire can list them; none is built in this version, so resolving one
//! answers [`ResolveError::NotBuilt`].

use hertzforge_mimo_shock::{MimoShockLawFactory, TargetKind};
use std::fmt;
use std::sync::Arc;

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
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Unknown(id) => write!(f, "no MIMO Shock law has the id `{id}`"),
            ResolveError::NotBuilt(id) => {
                write!(f, "the MIMO Shock law `{id}` cannot run in this version")
            }
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
