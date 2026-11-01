//! Native Eigen data: capture/recording folders (telemetry.f32 + capture.json)
//! from EVERY module namespace, and spectral snapshots (snapshot_*.json)
//! (the snapshot contract is the one the capture writers emit).

use crate::{ImportError, ImportResult};
use eigen_capture::{CaptureSidecar, SIDECAR_FILE, TELEMETRY_FILE};
use eigen_channel_table::Units;
use eigen_data::{
    units, AbsTime, Abscissa, FnId, FunctionKind, FunctionRecord, Ordinate, Provenance, SourceRef,
    Unit,
};
use std::path::Path;

/// A path is a capture/recording when it is (or contains) the sidecar pair.
pub fn sniff_capture(path: &str) -> bool {
    let p = Path::new(path);
    if p.is_dir() {
        return p.join(SIDECAR_FILE).is_file() && p.join(TELEMETRY_FILE).is_file();
    }
    p.file_name().map(|n| n == SIDECAR_FILE).unwrap_or(false)
        && p.parent()
            .map(|d| d.join(TELEMETRY_FILE).is_file())
            .unwrap_or(false)
}

pub fn sniff_snapshot(head: &[u8], path: &str) -> bool {
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    name.starts_with("snapshot_")
        && name.ends_with(".json")
        && head.trim_ascii_start().starts_with(b"{")
}

fn unit_from_table(u: &Units) -> Unit {
    match u {
        Units::AccelG => units::unit("g").unwrap(),
        Units::VelocityIps => units::unit("in/s").unwrap(),
        Units::DisplacementMil => units::unit("mil").unwrap(),
        Units::ForceLbf => units::unit("lbf").unwrap(),
        Units::VoltageV => units::unit("V").unwrap(),
        Units::Custom(s) => units::unit(s).unwrap_or_else(|| Unit::unknown(s)),
    }
}

/// Which module namespace a data-root path sits under ("recorder", "random",
/// "shock_classical", "shock_srs", "sine", "twr", or "explorer" for the root).
fn module_of(path: &Path) -> String {
    let comps: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    for m in [
        "recorder",
        "random",
        "shock_classical",
        "shock_srs",
        "sine",
        "twr",
        "smart",
    ] {
        if comps.contains(&m) {
            return if *"smart" == *m {
                "explorer".into()
            } else {
                m.to_string()
            };
        }
    }
    "explorer".into()
}

/// Import a capture/recording folder: one TimeHistory per recorded channel,
/// named from the AS-RECORDED channel table (never a bare channel number).
pub fn import_capture(
    dir: &Path,
    progress: &mut dyn FnMut(usize, u64, u64),
    cancel: &dyn Fn() -> bool,
) -> Result<ImportResult, ImportError> {
    let sidecar_path = dir.join(SIDECAR_FILE);
    let telemetry_path = dir.join(TELEMETRY_FILE);
    let sidecar_text =
        std::fs::read_to_string(&sidecar_path).map_err(|e| ImportError::Unreadable {
            detail: format!("{}: {e}", sidecar_path.display()),
        })?;
    let sidecar: CaptureSidecar =
        serde_json::from_str(&sidecar_text).map_err(|e| ImportError::Malformed {
            detail: format!("capture.json: {e}"),
            offset: None,
        })?;
    let mut warnings = Vec::new();
    if !sidecar.complete {
        warnings.push(
            "recording is marked incomplete (crash or live capture) - importing what was flushed"
                .into(),
        );
    }
    let bytes = std::fs::read(&telemetry_path).map_err(|e| ImportError::Unreadable {
        detail: format!("{}: {e}", telemetry_path.display()),
    })?;
    let n_ch = sidecar.config.n_channels.max(1) as usize;
    let total_vals = bytes.len() / 4;
    let n_frames = total_vals / n_ch;
    if bytes.len() % 4 != 0 {
        warnings.push(format!(
            "telemetry.f32 has {} residue bytes (truncated tail dropped)",
            bytes.len() % 4
        ));
    }
    if total_vals % n_ch != 0 {
        warnings.push(
            "telemetry length is not a whole number of frames (last partial frame dropped)".into(),
        );
    }
    // Channel slot -> table channel, via the layout's channel_order.
    let order: Vec<u32> = if sidecar.layout.channel_order.len() == n_ch {
        sidecar.layout.channel_order.clone()
    } else {
        warnings.push(
            "sidecar layout channel_order missing/short - assuming table-enabled order".into(),
        );
        sidecar
            .channel_table
            .channels
            .iter()
            .filter(|c| c.enabled)
            .map(|c| c.index)
            .take(n_ch)
            .collect()
    };
    let fs = sidecar.config.sample_rate_hz;
    let module = module_of(dir);
    let rec_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("recording");
    let mut de: Vec<Vec<f64>> = vec![Vec::with_capacity(n_frames); n_ch];
    let total_bytes = bytes.len() as u64;
    for f in 0..n_frames {
        if f % 65536 == 0 {
            progress(0, (f * n_ch * 4) as u64, total_bytes);
            if cancel() {
                return Err(ImportError::Malformed {
                    detail: "cancelled".into(),
                    offset: None,
                });
            }
        }
        let base = f * n_ch * 4;
        for (s, chan) in de.iter_mut().enumerate() {
            let off = base + s * 4;
            let v = f32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
            chan.push(v as f64);
        }
    }
    let t0 = Some(AbsTime {
        epoch_s: sidecar.start_unix_secs as i64,
        frac: 0.0,
    });
    let mut functions = Vec::with_capacity(n_ch);
    for (slot, series) in de.into_iter().enumerate() {
        let table_ch = order.get(slot).and_then(|idx| {
            sidecar
                .channel_table
                .channels
                .iter()
                .find(|c| c.index == *idx)
        });
        let (desc, unit) = match table_ch {
            Some(c) => {
                let d = if c.description.trim().is_empty() {
                    format!("input {}", c.index)
                } else {
                    c.description.clone()
                };
                (d, unit_from_table(&c.units))
            }
            None => {
                warnings.push(format!(
                    "slot {slot} has no channel-table row - unit unknown"
                ));
                (format!("input {slot}"), Unit::unknown(""))
            }
        };
        functions.push(FunctionRecord {
            id: FnId(slot as u64 + 1),
            name: format!("{module} {rec_name} {desc}"),
            role: table_ch.map(|c| format!("{:?}", c.role).to_lowercase()),
            response: None,
            reference: None,
            kind: FunctionKind::TimeHistory,
            abscissa: Abscissa::Even {
                start: 0.0,
                step: 1.0 / fs,
                n: n_frames,
            },
            ordinate: Ordinate::Real(series),
            unit_ordinate: unit,
            unit_abscissa: units::unit("s").unwrap(),
            t0,
            provenance: Provenance::Imported {
                format: "eigen-capture".into(),
                path: dir.display().to_string(),
                detail: format!("{fs} Hz, {n_frames} frames"),
            },
            source: SourceRef {
                path: dir.display().to_string(),
                format: "eigen-capture".into(),
                imported_at: 0,
            },
            comments: vec![],
        });
    }
    progress(functions.len(), total_bytes, total_bytes);
    Ok(ImportResult {
        functions,
        warnings,
    })
}

/// Import a spectral snapshot (B11 contract): mines the JSON for spectra
/// arrays `{ch?, f0_hz, df_hz, mags[]}`; kind per module namespace mapping;
/// anything unmappable imports as `Unknown` and the preview sheet assigns.
pub fn import_snapshot(path: &Path) -> Result<ImportResult, ImportError> {
    let text = std::fs::read_to_string(path).map_err(|e| ImportError::Unreadable {
        detail: e.to_string(),
    })?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| ImportError::Malformed {
        detail: e.to_string(),
        offset: None,
    })?;
    let mut warnings = Vec::new();
    let module = module_of(path);
    let (kind, unit_label): (FunctionKind, &str) = match module.as_str() {
        "random" => (FunctionKind::Psd, "g"),
        "explorer" | "sine" => (FunctionKind::LinearSpectrum, "g"),
        _ => (FunctionKind::Unknown, ""),
    };
    if kind == FunctionKind::Unknown {
        warnings.push(format!(
            "snapshot module '{module}': spectra imported as Unknown - assign kind and unit before comparing"
        ));
    }
    // Channel descriptions from settings_json, when the module stored a table.
    let mut desc_by_index: std::collections::HashMap<u64, String> = Default::default();
    if let Some(settings) = v.get("settings_json").and_then(|s| s.as_str()) {
        if let Ok(sv) = serde_json::from_str::<serde_json::Value>(settings) {
            collect_channel_descs(&sv, &mut desc_by_index);
        }
    }
    let mut spectra = Vec::new();
    collect_spectra(&v, &mut spectra);
    if spectra.is_empty() {
        return Err(ImportError::Malformed {
            detail: "snapshot carries no spectra arrays this importer recognizes".into(),
            offset: None,
        });
    }
    let t0 = v
        .get("unix_secs")
        .and_then(|u| u.as_i64())
        .map(|s| AbsTime {
            epoch_s: s,
            frac: 0.0,
        });
    let stem = path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("snapshot");
    let ord_unit = if unit_label.is_empty() {
        Unit::unknown("")
    } else {
        units::unit(unit_label).unwrap()
    };
    let mut functions = Vec::new();
    for (i, s) in spectra.into_iter().enumerate() {
        let label = match s.ch.and_then(|c| desc_by_index.get(&c).cloned()) {
            Some(d) => d,
            None => match s.ch {
                // A real per-module channel number is file data, not a
                // synthetic id - "input N" names the physical row honestly.
                Some(c) if c < 100 => format!("input {c}"),
                Some(c) => {
                    warnings.push(format!(
                        "trace id {c} is a module-synthetic overlay id - name it in the preview sheet"
                    ));
                    format!("overlay {c}")
                }
                None => format!("trace {}", i + 1),
            },
        };
        let n = s.mags.len();
        functions.push(FunctionRecord {
            id: FnId(i as u64 + 1),
            name: format!("{module} {stem} {label}"),
            role: None,
            response: None,
            reference: None,
            kind,
            abscissa: Abscissa::Even {
                start: s.f0_hz,
                step: s.df_hz,
                n,
            },
            ordinate: Ordinate::Real(s.mags),
            unit_ordinate: ord_unit.clone(),
            unit_abscissa: units::unit("Hz").unwrap(),
            t0,
            provenance: Provenance::Imported {
                format: "eigen-snapshot".into(),
                path: path.display().to_string(),
                detail: format!("{module} snapshot"),
            },
            source: SourceRef {
                path: path.display().to_string(),
                format: "eigen-snapshot".into(),
                imported_at: 0,
            },
            comments: vec![],
        });
    }
    Ok(ImportResult {
        functions,
        warnings,
    })
}

struct SnapSpectrum {
    ch: Option<u64>,
    f0_hz: f64,
    df_hz: f64,
    mags: Vec<f64>,
}

fn collect_spectra(v: &serde_json::Value, out: &mut Vec<SnapSpectrum>) {
    match v {
        serde_json::Value::Array(items) => {
            for it in items {
                if let Some(s) = as_spectrum(it) {
                    out.push(s);
                } else {
                    collect_spectra(it, out);
                }
            }
        }
        serde_json::Value::Object(map) => {
            for val in map.values() {
                collect_spectra(val, out);
            }
        }
        _ => {}
    }
}

fn as_spectrum(v: &serde_json::Value) -> Option<SnapSpectrum> {
    let o = v.as_object()?;
    let mags = o.get("mags")?.as_array()?;
    let f0 = o.get("f0_hz").and_then(|x| x.as_f64())?;
    let df = o.get("df_hz").and_then(|x| x.as_f64())?;
    if df <= 0.0 || mags.is_empty() {
        return None;
    }
    Some(SnapSpectrum {
        ch: o.get("ch").and_then(|c| c.as_u64()),
        f0_hz: f0,
        df_hz: df,
        mags: mags.iter().filter_map(|m| m.as_f64()).collect(),
    })
}

fn collect_channel_descs(v: &serde_json::Value, out: &mut std::collections::HashMap<u64, String>) {
    match v {
        serde_json::Value::Array(items) => {
            for it in items {
                collect_channel_descs(it, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let (Some(idx), Some(desc)) = (
                map.get("index").and_then(|i| i.as_u64()),
                map.get("description").and_then(|d| d.as_str()),
            ) {
                if !desc.trim().is_empty() {
                    // Table indices are 0-based; snapshot spectra channels are
                    // 1-based - store both keys so either lookup lands.
                    out.entry(idx + 1)
                        .or_insert_with(|| desc.trim().to_string());
                    out.entry(idx).or_insert_with(|| desc.trim().to_string());
                }
            }
            for val in map.values() {
                collect_channel_descs(val, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_mining_finds_spectra_and_names_channels() {
        let dir = std::env::temp_dir().join("eigen_fmt_snap_test/random");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("snapshot_1700000000.json");
        let settings = serde_json::json!({
            "channels": [
                {"index": 0, "description": "head accel 101X+"},
                {"index": 1, "description": "base ref 1Z+"}
            ]
        })
        .to_string();
        let snap = serde_json::json!({
            "unix_secs": 1700000000u64,
            "settings_json": settings,
            "marks": [],
            "payload": {
                "spectra": [
                    {"ch": 1, "f0_hz": 0.0, "df_hz": 2.5, "mags": [0.1, 0.2, 0.3]},
                    {"ch": 901, "f0_hz": 0.0, "df_hz": 2.5, "mags": [1.0, 1.0, 1.0]}
                ]
            }
        });
        std::fs::write(&p, serde_json::to_string(&snap).unwrap()).unwrap();
        let out = import_snapshot(&p).unwrap();
        assert_eq!(out.functions.len(), 2);
        assert_eq!(out.functions[0].kind, FunctionKind::Psd);
        assert!(
            out.functions[0].name.contains("head accel 101X+"),
            "{}",
            out.functions[0].name
        );
        assert!(out.functions[1].name.contains("overlay 901"));
        assert!(out.warnings.iter().any(|w| w.contains("synthetic")));
        assert_eq!(out.functions[0].t0.unwrap().epoch_s, 1700000000);
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }

    #[test]
    fn snapshot_from_unmapped_module_is_unknown_kind() {
        let dir = std::env::temp_dir().join("eigen_fmt_snap_test2/shock_srs");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("snapshot_5.json");
        let snap = serde_json::json!({
            "unix_secs": 5,
            "payload": {"spectra": [{"ch": 925, "f0_hz": 10.0, "df_hz": 5.0, "mags": [1.0, 2.0]}]}
        });
        std::fs::write(&p, serde_json::to_string(&snap).unwrap()).unwrap();
        let out = import_snapshot(&p).unwrap();
        assert_eq!(out.functions[0].kind, FunctionKind::Unknown);
        assert!(out.warnings.iter().any(|w| w.contains("assign kind")));
        std::fs::remove_dir_all(dir.parent().unwrap()).ok();
    }
}
