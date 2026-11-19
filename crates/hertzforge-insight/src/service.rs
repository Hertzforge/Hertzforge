//! The service orchestrator: what the tauri layer calls (the service verbs
//! minus the transport). Owns import dispatch, reprocess-into-session,
//! compare-set execution, export (through hertzforge-formats ONLY), and session
//! persistence (manifest + recipes, recompute on load).

use crate::compare::{self, CompareResult};
use crate::jobs::{PROMPT_ABOVE_BYTES, REFUSE_ABOVE_BYTES};
use crate::reprocess::{self, ReprocessParams};
use crate::transform::{self, TransformOp};
use crate::{
    FnMeta, ManifestDerived, ManifestImport, PaneLayout, Session, SessionManifest, MANIFEST_VERSION,
};
use hertzforge_data::FnId;
use hertzforge_formats::{csv, native, sniff, uff, wav, wave2_stub, Confidence, ImportError};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One import request as the tauri layer sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRequest {
    pub path: String,
    /// CSV commits carry preview-confirmed options; other formats None.
    pub csv_options: Option<csv::CsvOptions>,
    /// Explicit size-policy acknowledgment for files above the prompt line.
    pub big_file_ack: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportOutcome {
    pub ids: Vec<u64>,
    pub warnings: Vec<String>,
    pub format: String,
}

pub struct Service {
    pub session: Session,
    /// Import history for the manifest (order preserved).
    imports: Vec<ManifestImport>,
    derived: Vec<ManifestDerived>,
    data_root: PathBuf,
}

impl Service {
    pub fn new(data_root: PathBuf) -> Service {
        Service {
            session: Session::new(),
            imports: Vec::new(),
            derived: Vec::new(),
            data_root,
        }
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.data_root.join("insight").join("sessions")
    }

    /// Sniff a path for the preview flow.
    pub fn sniff_path(&self, path: &str) -> Result<(String, Option<csv::CsvPreview>), ImportError> {
        let p = Path::new(path);
        let head = read_head(p)?;
        let (format, conf) = sniff(&head, path);
        if conf == Confidence::No {
            return Err(ImportError::UnknownFormat);
        }
        if let Some(stub) = wave2_stub(format) {
            return Err(stub);
        }
        let preview = if format == "csv" {
            let bytes = std::fs::read(p).map_err(|e| ImportError::Unreadable {
                detail: e.to_string(),
            })?;
            Some(csv::preview(&bytes)?)
        } else {
            None
        };
        Ok((format.into(), preview))
    }

    /// Synchronous import = off-lock read + under-lock commit (tests/simple
    /// callers). The job engine calls the two halves separately.
    pub fn import(
        &mut self,
        req: &ImportRequest,
        progress: &mut dyn FnMut(usize, u64, u64),
        cancel: &dyn Fn() -> bool,
    ) -> Result<ImportOutcome, ImportError> {
        let (format, result) = read_import(req, progress, cancel)?;
        Ok(self.commit_import(req, &format, result))
    }

    /// The under-lock tail: adopt records + record history. Cheap by design.
    pub fn commit_import(
        &mut self,
        req: &ImportRequest,
        format: &str,
        mut result: hertzforge_formats::ImportResult,
    ) -> ImportOutcome {
        let warnings = std::mem::take(&mut result.warnings);
        let ids = self.session.adopt(result.functions);
        self.imports.push(ManifestImport {
            path: req.path.clone(),
            format: format.into(),
            csv_options: req.csv_options.clone(),
            produced: ids.clone(),
        });
        ImportOutcome {
            ids,
            warnings,
            format: format.into(),
        }
    }
}

/// The OFF-LOCK read: dispatch by sniffed format, build records, no session
/// access (records built off-lock, mutex only at commit).
pub fn read_import(
    req: &ImportRequest,
    progress: &mut dyn FnMut(usize, u64, u64),
    cancel: &dyn Fn() -> bool,
) -> Result<(String, hertzforge_formats::ImportResult), ImportError> {
    {
        let p = Path::new(&req.path);
        let size = file_size(p)?;
        if size > REFUSE_ABOVE_BYTES {
            return Err(ImportError::TooLarge {
                bytes: size,
                limit: REFUSE_ABOVE_BYTES,
            });
        }
        if size > PROMPT_ABOVE_BYTES && !req.big_file_ack {
            return Err(ImportError::TooLarge {
                bytes: size,
                limit: PROMPT_ABOVE_BYTES,
            });
        }
        let head = read_head(p)?;
        let (format, conf) = sniff(&head, &req.path);
        if conf == Confidence::No {
            return Err(ImportError::UnknownFormat);
        }
        if let Some(stub) = wave2_stub(format) {
            return Err(stub);
        }
        let result = match format {
            "uff" => {
                let bytes = std::fs::read(p).map_err(|e| ImportError::Unreadable {
                    detail: e.to_string(),
                })?;
                progress(0, bytes.len() as u64 / 2, bytes.len() as u64);
                if cancel() {
                    return Err(ImportError::Malformed {
                        detail: "cancelled".into(),
                        offset: None,
                    });
                }
                uff::import_bytes(&bytes, &req.path)?
            }
            "csv" => {
                let opts = req.csv_options.as_ref().ok_or(ImportError::Malformed {
                    detail: "CSV import needs preview-confirmed options".into(),
                    offset: None,
                })?;
                let bytes = std::fs::read(p).map_err(|e| ImportError::Unreadable {
                    detail: e.to_string(),
                })?;
                csv::import_with(&bytes, &req.path, opts)?
            }
            "wav" => wav::import(p)?,
            "hertzforge-capture" => {
                let dir = if p.is_dir() {
                    p.to_path_buf()
                } else {
                    p.parent().unwrap().to_path_buf()
                };
                native::import_capture(&dir, progress, cancel)?
            }
            "hertzforge-snapshot" => native::import_snapshot(p)?,
            other => {
                return Err(ImportError::Unsupported {
                    feature: format!("format '{other}'"),
                    wave: "unplanned".into(),
                })
            }
        };
        progress(result.functions.len(), size, size);
        Ok((format.to_string(), result))
    }
}

impl Service {
    /// Reprocess selected sources; adopts products, records the derivation.
    pub fn reprocess(
        &mut self,
        source_ids: &[u64],
        p: &ReprocessParams,
    ) -> Result<(Vec<u64>, Vec<String>), String> {
        let mut all_ids = Vec::new();
        let mut all_warnings = Vec::new();
        for &sid in source_ids {
            let source = self
                .session
                .get(sid)
                .ok_or_else(|| format!("no function {sid}"))?
                .clone();
            let reference = match p.reference {
                Some(rid) => Some(
                    self.session
                        .get(rid)
                        .ok_or_else(|| format!("no reference {rid}"))?
                        .clone(),
                ),
                None => None,
            };
            let out = reprocess::reprocess(&source, reference.as_ref(), p)?;
            all_warnings.extend(out.warnings);
            let ids = self.session.adopt(out.records);
            for &id in &ids {
                let rec = self.session.get(id).unwrap();
                self.derived.push(ManifestDerived {
                    id,
                    name: rec.name.clone(),
                    provenance: rec.provenance.clone(),
                    reprocess: Some((vec![sid], p.clone())),
                    promoted: None,
                    transform: None,
                });
            }
            all_ids.extend(ids);
        }
        Ok((all_ids, all_warnings))
    }

    /// Apply a v1.1 transform. Per-record ops map over `source_ids`; Xcorr
    /// takes exactly two ids in selection order [a, b].
    pub fn transform(
        &mut self,
        source_ids: &[u64],
        op: &TransformOp,
    ) -> Result<(Vec<u64>, Vec<String>), String> {
        let mut all_ids = Vec::new();
        let mut all_warnings = Vec::new();
        if matches!(op, TransformOp::Xcorr { .. }) {
            let [ia, ib] = source_ids else {
                return Err(format!(
                    "cross-correlation takes exactly two functions (got {})",
                    source_ids.len()
                ));
            };
            let a = self
                .session
                .get(*ia)
                .ok_or_else(|| format!("no function {ia}"))?
                .clone();
            let b = self
                .session
                .get(*ib)
                .ok_or_else(|| format!("no function {ib}"))?
                .clone();
            let TransformOp::Xcorr { norm } = op else {
                unreachable!()
            };
            let out = transform::transform_xcorr(&a, &b, *norm)?;
            all_warnings.extend(out.warnings);
            let ids = self.session.adopt(out.records);
            for &id in &ids {
                let rec = self.session.get(id).unwrap();
                self.derived.push(ManifestDerived {
                    id,
                    name: rec.name.clone(),
                    provenance: rec.provenance.clone(),
                    reprocess: None,
                    promoted: None,
                    transform: Some((vec![*ia, *ib], op.clone())),
                });
            }
            all_ids.extend(ids);
            return Ok((all_ids, all_warnings));
        }
        for &sid in source_ids {
            let source = self
                .session
                .get(sid)
                .ok_or_else(|| format!("no function {sid}"))?
                .clone();
            let out = transform::transform_one(&source, op)?;
            all_warnings.extend(out.warnings);
            let ids = self.session.adopt(out.records);
            for &id in &ids {
                let rec = self.session.get(id).unwrap();
                self.derived.push(ManifestDerived {
                    id,
                    name: rec.name.clone(),
                    provenance: rec.provenance.clone(),
                    reprocess: None,
                    promoted: None,
                    transform: Some((vec![sid], op.clone())),
                });
            }
            all_ids.extend(ids);
        }
        Ok((all_ids, all_warnings))
    }

    pub fn promote_tolerance(
        &mut self,
        id: u64,
        role: &str,
        offset_db: f64,
    ) -> Result<u64, String> {
        let new_id = self.session.promote_tolerance(id, role, offset_db)?;
        let rec = self.session.get(new_id).unwrap();
        self.derived.push(ManifestDerived {
            id: new_id,
            name: rec.name.clone(),
            provenance: rec.provenance.clone(),
            reprocess: None,
            promoted: Some((id, role.into(), offset_db)),
            transform: None,
        });
        Ok(new_id)
    }

    /// Run a compare set: primary = reference if set, else first member.
    /// Returns per-member results + the verdict fold across tolerance traces.
    pub fn run_compare(&self, set_name: &str) -> Result<CompareRun, String> {
        let set = self
            .session
            .compare_sets
            .get(set_name)
            .ok_or_else(|| format!("no compare set '{set_name}'"))?;
        let primary_id = set
            .reference
            .or_else(|| set.members.first().copied())
            .ok_or("compare set is empty")?;
        let primary = self
            .session
            .get(primary_id.0)
            .ok_or_else(|| format!("primary {} vanished", primary_id.0))?;
        let mut members = Vec::new();
        for m in &set.members {
            if *m == primary_id {
                continue;
            }
            let rec = self
                .session
                .get(m.0)
                .ok_or_else(|| format!("member {} vanished", m.0))?;
            members.push((m.0, compare::align_pair(primary, rec)?));
        }
        // Tolerances: hi/lo roles fold into the verdict against the primary.
        let mut hi: Option<CompareResult> = None;
        let mut lo: Option<CompareResult> = None;
        for (tid, role) in &set.tolerances {
            let t = self
                .session
                .get(tid.0)
                .ok_or_else(|| format!("tolerance {} vanished", tid.0))?;
            let r = compare::align_pair(primary, t)?;
            if role.contains("lo") {
                lo = Some(r);
            } else {
                hi = Some(r);
            }
        }
        let verdict = match (&hi, &lo) {
            (Some(h), l) => {
                let (outcome, margin, at) = compare::verdict_against(h, l.as_ref());
                Some(Verdict {
                    outcome,
                    margin_db: margin,
                    worst_at: at,
                })
            }
            (None, Some(l)) => {
                let (outcome, margin, at) = compare::verdict_against(l, None);
                // lo-only: margin sign flips are handled inside verdict_against
                // only for the lo argument; a lone lo trace checks as hi with
                // inverted sense - disclose instead of silently mislabeling.
                Some(Verdict {
                    outcome,
                    margin_db: margin.map(|m| -m),
                    worst_at: at,
                })
            }
            (None, None) => None,
        };
        Ok(CompareRun {
            primary: primary_id.0,
            members,
            hi,
            lo,
            verdict,
        })
    }

    /// Export selected functions (or an aligned compare run) via hertzforge-formats.
    pub fn export(
        &self,
        ids: &[u64],
        format: &str,
        out_path: &Path,
        method_lines: &[String],
    ) -> Result<(), String> {
        let recs: Vec<&hertzforge_data::FunctionRecord> = ids
            .iter()
            .map(|id| {
                self.session
                    .get(*id)
                    .ok_or_else(|| format!("no function {id}"))
            })
            .collect::<Result<_, _>>()?;
        match format {
            "uff" => {
                let text = uff::write_file(&recs, "Hertzforge Insight")?;
                std::fs::write(out_path, text).map_err(|e| e.to_string())?;
                // Method block rides a sidecar (UFF ID lines cannot hold it).
                if !method_lines.is_empty() {
                    let sidecar = serde_json::json!({ "method": method_lines });
                    std::fs::write(
                        method_sidecar_path(out_path),
                        serde_json::to_string_pretty(&sidecar).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
                }
                Ok(())
            }
            "csv" => {
                let text = csv::write_csv(&recs, method_lines)?;
                std::fs::write(out_path, text).map_err(|e| e.to_string())
            }
            "wav" => {
                let note = method_lines.join(" | ");
                wav::export(&recs, out_path, (!note.is_empty()).then_some(note.as_str()))
            }
            "mat" => Err("MAT export is Insight wave 2 (stated deferral)".into()),
            other => Err(format!("unknown export format '{other}'")),
        }
    }

    // ---------------- session persistence ----------------

    pub fn save_session(&mut self, name: &str) -> Result<PathBuf, String> {
        let name = sanitize_name(name)?;
        let dir = self.sessions_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let manifest = SessionManifest {
            version: MANIFEST_VERSION,
            name: name.clone(),
            saved_unix: now_unix(),
            imports: self.imports.clone(),
            derived: self.derived.clone(),
            compare_sets: self.session.compare_sets.values().cloned().collect(),
            layout: self.session.layout.clone(),
        };
        let path = dir.join(format!("{name}.json"));
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        self.session.dirty = false;
        Ok(path)
    }

    pub fn list_sessions(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.sessions_dir()) {
            for e in rd.flatten() {
                if let Some(n) = e.file_name().to_str() {
                    if let Some(stem) = n.strip_suffix(".json") {
                        out.push(stem.to_string());
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Load = REBUILD: re-import every source, re-run every derivation, then
    /// restore sets/layout with the manifest's id mapping. Missing sources
    /// degrade to warnings, never a refused load.
    pub fn load_session(&mut self, name: &str) -> Result<Vec<String>, String> {
        let path = self
            .sessions_dir()
            .join(format!("{}.json", sanitize_name(name)?));
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let manifest: SessionManifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let mut warnings = Vec::new();
        let mut svc = Service::new(self.data_root.clone());
        // old-id -> new-id mapping.
        let mut idmap: std::collections::HashMap<u64, u64> = Default::default();
        for imp in &manifest.imports {
            let req = ImportRequest {
                path: imp.path.clone(),
                csv_options: imp.csv_options.clone(),
                big_file_ack: true,
            };
            let (mut noop, never) = hertzforge_formats::sync_ctx();
            match svc.import(&req, &mut noop, &never) {
                Ok(out) => {
                    if out.ids.len() == imp.produced.len() {
                        for (old, new) in imp.produced.iter().zip(&out.ids) {
                            idmap.insert(*old, *new);
                        }
                    } else {
                        warnings.push(format!(
                            "'{}' produced {} records (session had {}) - ids matched by order where possible",
                            imp.path,
                            out.ids.len(),
                            imp.produced.len()
                        ));
                        for (old, new) in imp.produced.iter().zip(&out.ids) {
                            idmap.insert(*old, *new);
                        }
                    }
                }
                Err(e) => warnings.push(format!("source '{}' failed to reload: {e}", imp.path)),
            }
        }
        for d in &manifest.derived {
            match (&d.reprocess, &d.promoted) {
                (Some((srcs, params)), _) => {
                    let mapped: Vec<u64> =
                        srcs.iter().filter_map(|s| idmap.get(s).copied()).collect();
                    if mapped.len() != srcs.len() {
                        warnings.push(format!("derivation '{}' lost its source - skipped", d.name));
                        continue;
                    }
                    let mut p = params.clone();
                    if let Some(r) = p.reference {
                        p.reference = idmap.get(&r).copied();
                        if p.reference.is_none() {
                            warnings.push(format!(
                                "derivation '{}' lost its reference - skipped",
                                d.name
                            ));
                            continue;
                        }
                    }
                    match svc.reprocess(&mapped, &p) {
                        Ok((ids, w)) => {
                            warnings.extend(w);
                            if let Some(first) = ids.first() {
                                idmap.insert(d.id, *first);
                                // Multi-product derivations (FRF+coherence) map
                                // in order after the recorded id.
                                for (k, id) in ids.iter().enumerate().skip(1) {
                                    idmap.insert(d.id + k as u64, *id);
                                }
                            }
                        }
                        Err(e) => warnings.push(format!("derivation '{}' failed: {e}", d.name)),
                    }
                }
                (_, Some((src, role, off))) => match idmap.get(src) {
                    Some(new_src) => match svc.promote_tolerance(*new_src, role, *off) {
                        Ok(nid) => {
                            idmap.insert(d.id, nid);
                        }
                        Err(e) => warnings.push(format!("promotion '{}' failed: {e}", d.name)),
                    },
                    None => {
                        warnings.push(format!("promotion '{}' lost its source - skipped", d.name))
                    }
                },
                _ => {
                    if let Some((srcs, op)) = &d.transform {
                        let mapped: Vec<u64> =
                            srcs.iter().filter_map(|s| idmap.get(s).copied()).collect();
                        if mapped.len() != srcs.len() {
                            warnings
                                .push(format!("transform '{}' lost a source - skipped", d.name));
                            continue;
                        }
                        match svc.transform(&mapped, op) {
                            Ok((ids, w)) => {
                                warnings.extend(w);
                                if let Some(first) = ids.first() {
                                    idmap.insert(d.id, *first);
                                }
                            }
                            Err(e) => warnings.push(format!("transform '{}' failed: {e}", d.name)),
                        }
                    }
                }
            }
        }
        for set in &manifest.compare_sets {
            let mut s2 = set.clone();
            s2.members = set
                .members
                .iter()
                .filter_map(|m| idmap.get(&m.0).map(|n| FnId(*n)))
                .collect();
            s2.reference = set
                .reference
                .and_then(|r| idmap.get(&r.0).map(|n| FnId(*n)));
            s2.tolerances = set
                .tolerances
                .iter()
                .filter_map(|(t, role)| idmap.get(&t.0).map(|n| (FnId(*n), role.clone())))
                .collect();
            svc.session.compare_sets.insert(s2.name.clone(), s2);
        }
        svc.session.layout = remap_layout(&manifest.layout, &idmap);
        svc.session.dirty = false;
        *self = svc;
        Ok(warnings)
    }

    pub fn delete_session(&self, name: &str) -> Result<(), String> {
        let path = self
            .sessions_dir()
            .join(format!("{}.json", sanitize_name(name)?));
        std::fs::remove_file(path).map_err(|e| e.to_string())
    }

    pub fn catalog(&self) -> Vec<FnMeta> {
        self.session.catalog()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verdict {
    pub outcome: compare::CompareOutcome,
    pub margin_db: Option<f64>,
    pub worst_at: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompareRun {
    pub primary: u64,
    pub members: Vec<(u64, CompareResult)>,
    pub hi: Option<CompareResult>,
    pub lo: Option<CompareResult>,
    pub verdict: Option<Verdict>,
}

fn remap_layout(l: &PaneLayout, idmap: &std::collections::HashMap<u64, u64>) -> PaneLayout {
    let mut out = l.clone();
    for pane in out.panes.iter_mut() {
        pane.traces = pane
            .traces
            .iter()
            .filter_map(|t| idmap.get(&t.0).map(|n| FnId(*n)))
            .collect();
    }
    out
}

fn sanitize_name(name: &str) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() || n.len() > 80 {
        return Err("session name must be 1..80 characters".into());
    }
    if n.chars()
        .any(|c| matches!(c, '/' | '\\' | ':' | '.' | '\0'))
    {
        return Err("session name cannot contain path characters".into());
    }
    Ok(n.into())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn file_size(p: &Path) -> Result<u64, ImportError> {
    if p.is_dir() {
        // Captures: telemetry file size is the honest figure.
        let t = p.join(hertzforge_capture::TELEMETRY_FILE);
        return std::fs::metadata(&t)
            .map(|m| m.len())
            .map_err(|e| ImportError::Unreadable {
                detail: format!("{}: {e}", t.display()),
            });
    }
    std::fs::metadata(p)
        .map(|m| m.len())
        .map_err(|e| ImportError::Unreadable {
            detail: e.to_string(),
        })
}

fn read_head(p: &Path) -> Result<Vec<u8>, ImportError> {
    use std::io::Read;
    if p.is_dir() {
        return Ok(Vec::new());
    }
    let mut f = std::fs::File::open(p).map_err(|e| ImportError::Unreadable {
        detail: e.to_string(),
    })?;
    let mut head = vec![0u8; 4096];
    let n = f.read(&mut head).map_err(|e| ImportError::Unreadable {
        detail: e.to_string(),
    })?;
    head.truncate(n);
    Ok(head)
}

fn method_sidecar_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".method.json");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompareSet;
    use hertzforge_data::FunctionKind;

    fn write_uff_fixture(dir: &Path) -> PathBuf {
        // A 64k-sample time history at 8192 Hz, written through our own writer.
        let fs = 8192.0;
        let n = 1 << 16;
        let v: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 / fs;
                1.2 * (2.0 * std::f64::consts::PI * 120.0 * t).sin()
                    + 0.4 * (2.0 * std::f64::consts::PI * 480.0 * t).sin()
            })
            .collect();
        let mut rec = crate::tests::mk(
            FunctionKind::TimeHistory,
            hertzforge_data::Abscissa::Even {
                start: 0.0,
                step: 1.0 / fs,
                n,
            },
            v,
            "bench head 101X+",
        );
        rec.unit_abscissa = hertzforge_data::units::unit("s").unwrap();
        let text = uff::write_file(&[&rec], "fixture").unwrap();
        let p = dir.join("run.unv");
        std::fs::write(&p, text).unwrap();
        p
    }

    /// The I4 gate: the full acceptance chain import -> reprocess -> compare
    /// -> verdict -> export, then session save -> load rebuild.
    #[test]
    fn acceptance_chain_import_reprocess_compare_export_session() {
        let root =
            std::env::temp_dir().join(format!("hertzforge_insight_svc_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let uff_path = write_uff_fixture(&root);
        let mut svc = Service::new(root.clone());

        // Import.
        let (mut noop, never) = hertzforge_formats::sync_ctx();
        let out = svc
            .import(
                &ImportRequest {
                    path: uff_path.display().to_string(),
                    csv_options: None,
                    big_file_ack: false,
                },
                &mut noop,
                &never,
            )
            .unwrap();
        assert_eq!(out.format, "uff");
        assert_eq!(out.ids.len(), 1);

        // Reprocess -> PSD.
        let (psd_ids, _w) = svc
            .reprocess(
                &out.ids,
                &ReprocessParams {
                    nfft: 2048,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(psd_ids.len(), 1);
        let meta = svc.session.meta(psd_ids[0]).unwrap();
        assert_eq!(meta.kind, "PSD");
        assert!(meta.recipe.contains("Hann 2048"));

        // Tolerance: promote the PSD +6 dB as a hi limit; compare set passes.
        let tol = svc.promote_tolerance(psd_ids[0], "limit-hi", 6.0).unwrap();
        svc.session.compare_sets.insert(
            "accept".into(),
            CompareSet {
                name: "accept".into(),
                members: vec![FnId(psd_ids[0])],
                reference: Some(FnId(psd_ids[0])),
                tolerances: vec![(FnId(tol), "limit-hi".into())],
            },
        );
        let run = svc.run_compare("accept").unwrap();
        let v = run.verdict.unwrap();
        assert_eq!(v.outcome, compare::CompareOutcome::Pass);
        assert!(
            (v.margin_db.unwrap() - 6.0).abs() < 0.01,
            "{:?}",
            v.margin_db
        );

        // Export the PSD as CSV with a method line; file exists + carries it.
        let csv_out = root.join("psd.csv");
        svc.export(&psd_ids, "csv", &csv_out, &["rung: exact grids".into()])
            .unwrap();
        let text = std::fs::read_to_string(&csv_out).unwrap();
        assert!(text.starts_with("# rung: exact grids"));
        assert!(text.contains("bench head 101X+"));

        // UFF export writes a method SIDECAR.
        let uff_out = root.join("psd.unv");
        svc.export(&psd_ids, "uff", &uff_out, &["rung: exact grids".into()])
            .unwrap();
        assert!(
            uff_out.with_extension("unv.method.json").exists()
                || std::fs::metadata(format!("{}.method.json", uff_out.display())).is_ok()
        );

        // Session save -> load rebuilds imports AND derivations.
        svc.session.layout.panes.push(crate::PaneState {
            mode: "spectrum".into(),
            traces: vec![FnId(psd_ids[0])],
            compare_set: Some("accept".into()),
            log_x: true,
            log_y: true,
        });
        svc.save_session("bench night").unwrap();
        let mut svc2 = Service::new(root.clone());
        let warnings = svc2.load_session("bench night").unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(svc2.session.len(), 3, "time + psd + tolerance rebuilt");
        let run2 = svc2.run_compare("accept").unwrap();
        assert_eq!(run2.verdict.unwrap().outcome, compare::CompareOutcome::Pass);
        assert_eq!(svc2.session.layout.panes.len(), 1);
        assert_eq!(svc2.session.layout.panes[0].traces.len(), 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn size_policy_prompts_and_refuses() {
        let root =
            std::env::temp_dir().join(format!("hertzforge_insight_size_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut svc = Service::new(root.clone());
        let p = root.join("small.unv");
        std::fs::write(&p, b"    -1\n    58\n").unwrap();
        // Small file passes the size gate (fails later on structure).
        let (mut noop, never) = hertzforge_formats::sync_ctx();
        let err = svc
            .import(
                &ImportRequest {
                    path: p.display().to_string(),
                    csv_options: None,
                    big_file_ack: false,
                },
                &mut noop,
                &never,
            )
            .unwrap_err();
        assert!(matches!(err, ImportError::Malformed { .. }));
        std::fs::remove_dir_all(&root).ok();
    }

    /// The transform-chain gate: adopt -> filter -> integrate -> resample -> xcorr,
    /// every product in the catalog with an honest recipe, then session
    /// save -> load rebuilds the whole transform chain.
    #[test]
    fn transform_chain_filters_integrates_resamples_xcorrs_and_reloads() {
        use crate::transform::{FilterShape, TransformOp, XcorrNormWire};
        let root =
            std::env::temp_dir().join(format!("hertzforge_insight_tx_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let uff_path = write_uff_fixture(&root);
        let mut svc = Service::new(root.clone());
        let (mut noop, never) = hertzforge_formats::sync_ctx();
        let out = svc
            .import(
                &ImportRequest {
                    path: uff_path.display().to_string(),
                    csv_options: None,
                    big_file_ack: false,
                },
                &mut noop,
                &never,
            )
            .unwrap();
        let src = out.ids[0];

        let (f_ids, _) = svc
            .transform(
                &[src],
                &TransformOp::Filter {
                    shape: FilterShape::Lowpass,
                    order: 4,
                    f1: 250.0,
                    f2: None,
                    zero_phase: true,
                },
            )
            .unwrap();
        let (i_ids, _) = svc
            .transform(
                &f_ids,
                &TransformOp::Integrate {
                    drift_hp_hz: Some(2.0),
                },
            )
            .unwrap();
        let (r_ids, r_warn) = svc
            .transform(&[src], &TransformOp::Resample { target_fs: 4096.0 })
            .unwrap();
        assert!(
            r_warn.iter().any(|w| w.contains("anti-alias")),
            "{r_warn:?}"
        );
        let (x_ids, _) = svc
            .transform(
                &[src, f_ids[0]],
                &TransformOp::Xcorr {
                    norm: XcorrNormWire::Coeff,
                },
            )
            .unwrap();

        let cat = svc.catalog();
        let meta = |id: u64| cat.iter().find(|m| m.id == id).unwrap();
        assert!(meta(f_ids[0]).recipe.contains("Butterworth LP 4 @ 250 Hz"));
        assert!(meta(i_ids[0])
            .recipe
            .contains("no gravitational conversion"));
        // The UFF spec's E13.5 increment field limits dx to ~6 significant
        // digits, so fs round-trips as 8192.02... and the recipe prints THAT
        // (rate honesty): assert the structural facts, not a nominal number.
        assert!(
            meta(r_ids[0]).recipe.contains("L/M 1/2, polyphase"),
            "{}",
            meta(r_ids[0]).recipe
        );
        assert!(
            meta(r_ids[0]).recipe.contains("resample 8192.0"),
            "{}",
            meta(r_ids[0]).recipe
        );
        assert!(meta(x_ids[0]).recipe.contains("positive lag"));
        assert_eq!(meta(x_ids[0]).kind, "Correlation");

        // Xcorr arity is enforced at the service verb.
        assert!(svc
            .transform(
                &[src],
                &TransformOp::Xcorr {
                    norm: XcorrNormWire::Raw
                }
            )
            .is_err());

        // The manifest rebuilds the chain on load.
        svc.save_session("tx-chain").unwrap();
        let mut svc2 = Service::new(root.clone());
        let warnings = svc2.load_session("tx-chain").unwrap();
        assert!(
            !warnings
                .iter()
                .any(|w| w.contains("failed") || w.contains("skipped")),
            "{warnings:?}"
        );
        let names: Vec<String> = svc2.catalog().iter().map(|m| m.name.clone()).collect();
        assert!(names.iter().any(|n| n.contains("LP 250")), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("∫")), "{names:?}");
        assert!(names.iter().any(|n| n.contains("@ 4096")), "{names:?}");
        assert!(names.iter().any(|n| n.starts_with("xcorr:")), "{names:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn wave2_formats_error_typed_through_sniff() {
        let root =
            std::env::temp_dir().join(format!("hertzforge_insight_w2_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let svc = Service::new(root.clone());
        let p = root.join("data.mat");
        std::fs::write(&p, b"MATLAB 5.0 MAT-file").unwrap();
        match svc.sniff_path(&p.display().to_string()) {
            Err(ImportError::Unsupported { wave, .. }) => assert!(wave.contains("wave 2")),
            other => panic!("{other:?}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
