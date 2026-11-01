//! `hertzforge-insight` - the Insight session service.
//!
//! Policy layer: knows what functions MEAN and which kernel applies. Holds the
//! session (functions, compare sets, profiles, pane layout), orchestrates
//! import/reprocess/compare/export, and produces plot payloads. No UI
//! knowledge, no tauri types - the tauri layer adapts.

pub mod compare;
pub mod jobs;
pub mod payload;
pub mod reprocess;
pub mod service;
pub mod transform;

use hertzforge_data::{FnId, FunctionRecord, Provenance};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub use compare::{CompareOutcome, CompareResult, RegridMethod};
pub use payload::TracePayload;
pub use reprocess::{ReprocessKind, ReprocessParams, WindowKind};
pub use transform::{FilterShape, TransformOp, XcorrNormWire};

/// A named compare set: operand traces + an optional reference + tolerance
/// traces promoted onto it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompareSet {
    pub name: String,
    pub members: Vec<FnId>,
    pub reference: Option<FnId>,
    /// (trace, role) - roles like "limit-hi" / "limit-lo".
    pub tolerances: Vec<(FnId, String)>,
}

/// Pane layout state (persisted with the session).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaneLayout {
    pub panes: Vec<PaneState>,
    pub active: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaneState {
    pub mode: String, // "time" | "spectrum" | "frf" | "coherence" | "srs"
    pub traces: Vec<FnId>,
    pub compare_set: Option<String>,
    pub log_x: bool,
    pub log_y: bool,
}

/// The in-memory session. All mutation goes through `Session` methods so the
/// id counter and provenance graph stay coherent.
#[derive(Debug, Default)]
pub struct Session {
    functions: BTreeMap<u64, FunctionRecord>,
    next_id: u64,
    pub compare_sets: BTreeMap<String, CompareSet>,
    pub layout: PaneLayout,
    pub dirty: bool,
}

/// Catalog row for the UI browser (names + metadata, never bulk data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FnMeta {
    pub id: u64,
    pub name: String,
    pub role: Option<String>,
    pub kind: String,
    pub kind_is_unknown: bool,
    pub n: usize,
    pub span: (f64, f64),
    pub unit_ordinate: String,
    pub unit_abscissa: String,
    pub response: Option<String>,
    pub reference: Option<String>,
    pub recipe: String,
    pub source_format: String,
    pub complex: bool,
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    /// Insert records from an import, re-basing their ids onto the session's
    /// monotonic counter (ids are session-scoped, NEVER reused).
    pub fn adopt(&mut self, mut records: Vec<FunctionRecord>) -> Vec<u64> {
        let mut ids = Vec::with_capacity(records.len());
        for mut r in records.drain(..) {
            self.next_id += 1;
            r.id = FnId(self.next_id);
            ids.push(self.next_id);
            self.functions.insert(self.next_id, r);
        }
        self.dirty = !ids.is_empty() || self.dirty;
        ids
    }

    pub fn get(&self, id: u64) -> Option<&FunctionRecord> {
        self.functions.get(&id)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        let hit = self.functions.remove(&id).is_some();
        if hit {
            for set in self.compare_sets.values_mut() {
                set.members.retain(|m| m.0 != id);
                set.tolerances.retain(|(t, _)| t.0 != id);
                if set.reference.map(|r| r.0) == Some(id) {
                    set.reference = None;
                }
            }
            for pane in self.layout.panes.iter_mut() {
                pane.traces.retain(|t| t.0 != id);
            }
            self.dirty = true;
        }
        hit
    }

    pub fn rename(&mut self, id: u64, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("name cannot be empty".into());
        }
        match self.functions.get_mut(&id) {
            Some(r) => {
                r.name = name.into();
                self.dirty = true;
                Ok(())
            }
            None => Err(format!("no function {id}")),
        }
    }

    /// Resolve an Unknown-kind import (the preview sheet's assignment verb).
    pub fn assign_kind(
        &mut self,
        id: u64,
        kind: hertzforge_data::FunctionKind,
        unit_label: &str,
    ) -> Result<(), String> {
        let r = self
            .functions
            .get_mut(&id)
            .ok_or_else(|| format!("no function {id}"))?;
        r.kind = kind;
        if !unit_label.trim().is_empty() {
            r.unit_ordinate = hertzforge_data::units::unit(unit_label)
                .unwrap_or_else(|| hertzforge_data::Unit::unknown(unit_label));
        }
        r.provenance = Provenance::Derived {
            op: "assign-kind".into(),
            params_json: format!(
                "{{\"kind\":\"{}\",\"unit\":\"{unit_label}\"}}",
                kind.label()
            ),
            from: vec![FnId(id)],
        };
        self.dirty = true;
        Ok(())
    }

    pub fn meta(&self, id: u64) -> Option<FnMeta> {
        self.functions.get(&id).map(|r| FnMeta {
            id,
            name: r.name.clone(),
            role: r.role.clone(),
            kind: r.kind.label().into(),
            kind_is_unknown: r.kind == hertzforge_data::FunctionKind::Unknown,
            n: r.ordinate.len(),
            span: (
                r.abscissa.first().unwrap_or(0.0),
                r.abscissa.last().unwrap_or(0.0),
            ),
            unit_ordinate: r.unit_ordinate.label.clone(),
            unit_abscissa: r.unit_abscissa.label.clone(),
            response: r.response.map(|d| d.to_string()),
            reference: r.reference.map(|d| d.to_string()),
            recipe: r.provenance.recipe_line(),
            source_format: r.source.format.clone(),
            complex: r.ordinate.is_complex(),
        })
    }

    pub fn catalog(&self) -> Vec<FnMeta> {
        self.functions
            .keys()
            .filter_map(|&id| self.meta(id))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.functions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Promote any trace to a tolerance role with a dB offset (the ObserVIEW
    /// pattern): creates a DERIVED record, offset applied in the power/
    /// amplitude form matching the source kind.
    pub fn promote_tolerance(
        &mut self,
        id: u64,
        role: &str,
        offset_db: f64,
    ) -> Result<u64, String> {
        let src = self
            .functions
            .get(&id)
            .ok_or_else(|| format!("no function {id}"))?;
        if src.ordinate.is_complex() {
            return Err("cannot promote a complex trace to a tolerance".into());
        }
        let factor = if src.kind.rebin_class() {
            hertzforge_dsp::db::from_db_power(offset_db, 1.0)
        } else {
            hertzforge_dsp::db::from_db_amplitude(offset_db, 1.0)
        };
        let mut rec = src.clone();
        let hertzforge_data::Ordinate::Real(v) = &mut rec.ordinate else {
            unreachable!()
        };
        for x in v.iter_mut() {
            *x *= factor;
        }
        rec.role = Some(role.into());
        rec.name = format!("{} [{role} {offset_db:+.1} dB]", src.name);
        rec.provenance = Provenance::Promoted {
            role: role.into(),
            offset_db,
            from: FnId(id),
        };
        let new_id = self.adopt(vec![rec]);
        Ok(new_id[0])
    }
}

/// Session manifest: sources + recipes + layout, derived traces
/// recomputed on load. v1 persists IMPORTED sources by path and DERIVED
/// records by recipe; bulk data never serializes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionManifest {
    pub version: u32,
    pub name: String,
    pub saved_unix: i64,
    pub imports: Vec<ManifestImport>,
    pub derived: Vec<ManifestDerived>,
    pub compare_sets: Vec<CompareSet>,
    pub layout: PaneLayout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestImport {
    pub path: String,
    pub format: String,
    /// CSV commits carry their preview-confirmed options.
    pub csv_options: Option<hertzforge_formats::csv::CsvOptions>,
    /// Ids the import produced, in order (stable re-adoption on load).
    pub produced: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestDerived {
    pub id: u64,
    pub name: String,
    pub provenance: Provenance,
    /// Reprocess parameters when the derivation was a reprocess.
    pub reprocess: Option<(Vec<u64>, ReprocessParams)>,
    pub promoted: Option<(u64, String, f64)>,
    /// Transform op when the derivation was a v1.1 transform (defaulted so
    /// pre-v1.1 session manifests load unchanged).
    #[serde(default)]
    pub transform: Option<(Vec<u64>, TransformOp)>,
}

pub const MANIFEST_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;
    use hertzforge_data::{units::unit, Abscissa, FunctionKind, Ordinate, SourceRef};

    pub(crate) fn mk(kind: FunctionKind, xs: Abscissa, ys: Vec<f64>, name: &str) -> FunctionRecord {
        FunctionRecord {
            id: FnId(0),
            name: name.into(),
            role: None,
            response: None,
            reference: None,
            kind,
            abscissa: xs,
            ordinate: Ordinate::Real(ys),
            unit_ordinate: unit("g").unwrap(),
            unit_abscissa: unit("Hz").unwrap(),
            t0: None,
            provenance: Provenance::Imported {
                format: "test".into(),
                path: "mem".into(),
                detail: String::new(),
            },
            source: SourceRef {
                path: "mem".into(),
                format: "test".into(),
                imported_at: 0,
            },
            comments: vec![],
        }
    }

    #[test]
    fn ids_are_monotonic_and_never_reused() {
        let mut s = Session::new();
        let a = s.adopt(vec![mk(
            FunctionKind::Psd,
            Abscissa::Even {
                start: 0.0,
                step: 1.0,
                n: 4,
            },
            vec![1.0; 4],
            "a",
        )]);
        s.remove(a[0]);
        let b = s.adopt(vec![mk(
            FunctionKind::Psd,
            Abscissa::Even {
                start: 0.0,
                step: 1.0,
                n: 4,
            },
            vec![1.0; 4],
            "b",
        )]);
        assert!(b[0] > a[0], "removed id must never come back");
    }

    #[test]
    fn remove_scrubs_sets_and_panes() {
        let mut s = Session::new();
        let ids = s.adopt(vec![
            mk(
                FunctionKind::Psd,
                Abscissa::Even {
                    start: 0.0,
                    step: 1.0,
                    n: 4,
                },
                vec![1.0; 4],
                "a",
            ),
            mk(
                FunctionKind::Psd,
                Abscissa::Even {
                    start: 0.0,
                    step: 1.0,
                    n: 4,
                },
                vec![2.0; 4],
                "b",
            ),
        ]);
        s.compare_sets.insert(
            "cs".into(),
            CompareSet {
                name: "cs".into(),
                members: vec![FnId(ids[0]), FnId(ids[1])],
                reference: Some(FnId(ids[0])),
                tolerances: vec![(FnId(ids[0]), "limit-hi".into())],
            },
        );
        s.layout.panes.push(PaneState {
            mode: "spectrum".into(),
            traces: vec![FnId(ids[0])],
            compare_set: None,
            log_x: true,
            log_y: true,
        });
        s.remove(ids[0]);
        let cs = &s.compare_sets["cs"];
        assert_eq!(cs.members.len(), 1);
        assert!(cs.reference.is_none());
        assert!(cs.tolerances.is_empty());
        assert!(s.layout.panes[0].traces.is_empty());
    }

    #[test]
    fn promote_tolerance_applies_power_db_for_psd() {
        let mut s = Session::new();
        let ids = s.adopt(vec![mk(
            FunctionKind::Psd,
            Abscissa::Even {
                start: 10.0,
                step: 2.0,
                n: 3,
            },
            vec![0.04, 0.04, 0.04],
            "ref psd",
        )]);
        let tid = s.promote_tolerance(ids[0], "limit-hi", 6.0).unwrap();
        let t = s.get(tid).unwrap();
        let Ordinate::Real(v) = &t.ordinate else {
            panic!()
        };
        // +6 dB POWER on a PSD ~ x3.981
        assert!((v[0] / 0.04 - 10f64.powf(0.6)).abs() < 1e-9);
        assert_eq!(t.role.as_deref(), Some("limit-hi"));
        assert!(t.name.contains("limit-hi"));
    }

    #[test]
    fn assign_kind_resolves_unknown_and_records_provenance() {
        let mut s = Session::new();
        let ids = s.adopt(vec![mk(
            FunctionKind::Unknown,
            Abscissa::Even {
                start: 10.0,
                step: 5.0,
                n: 3,
            },
            vec![1.0, 2.0, 1.5],
            "mystery",
        )]);
        s.assign_kind(ids[0], FunctionKind::Srs, "g").unwrap();
        let r = s.get(ids[0]).unwrap();
        assert_eq!(r.kind, FunctionKind::Srs);
        assert!(r.provenance.recipe_line().contains("assign-kind"));
    }
}
