//! WAV import/export via `hound` (Apache-2.0), with the calibration sidecar
//! that gives vibration recordings their units back (WAV
//! carries NO engineering units - the sidecar is not
//! optional bookkeeping, it is what makes the data mean anything).
//!
//! Sidecar: `<file>.eigencal.json` next to the WAV:
//! ```json
//! { "channels": [ { "name": "head accel 101X+", "unit": "g",
//!                   "eu_per_full_scale": 5.0 } ],
//!   "t0_epoch_s": 1700000000 }
//! ```
//! Integer samples normalize to [-1, 1] full scale, then scale by
//! `eu_per_full_scale`. Float WAV samples are taken as already-normalized
//! full scale. Without a sidecar, amplitudes import as fraction-of-full-scale
//! with a loud warning (never silently pretend volts).

use crate::{ImportError, ImportResult};
use eigen_data::{
    units, AbsTime, Abscissa, FnId, FunctionKind, FunctionRecord, Ordinate, Provenance, SourceRef,
    Unit,
};
use std::path::Path;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CalChannel {
    pub name: String,
    pub unit: String,
    pub eu_per_full_scale: f64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CalSidecar {
    #[serde(default)]
    pub channels: Vec<CalChannel>,
    #[serde(default)]
    pub t0_epoch_s: Option<i64>,
}

pub fn sidecar_path(wav: &Path) -> std::path::PathBuf {
    let mut s = wav.as_os_str().to_owned();
    s.push(".eigencal.json");
    std::path::PathBuf::from(s)
}

pub fn import(path: &Path) -> Result<ImportResult, ImportError> {
    let mut warnings = Vec::new();
    let reader = hound::WavReader::open(path).map_err(|e| ImportError::Malformed {
        detail: e.to_string(),
        offset: None,
    })?;
    let spec = reader.spec();
    let n_ch = spec.channels.max(1) as usize;
    let fs = spec.sample_rate as f64;

    let cal: CalSidecar = match std::fs::read_to_string(sidecar_path(path)) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| ImportError::Malformed {
            detail: format!("calibration sidecar: {e}"),
            offset: None,
        })?,
        Err(_) => {
            warnings.push(
                "no calibration sidecar (<file>.eigencal.json) - amplitudes are FRACTION OF FULL SCALE, not engineering units".into(),
            );
            CalSidecar::default()
        }
    };

    // Decode interleaved samples to normalized f64.
    let interleaved: Vec<f64> = match spec.sample_format {
        hound::SampleFormat::Float => {
            let mut r = reader;
            r.samples::<f32>()
                .map(|s| s.map(|v| v as f64))
                .collect::<Result<_, _>>()
                .map_err(|e| ImportError::Malformed {
                    detail: e.to_string(),
                    offset: None,
                })?
        }
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            let full = (1i64 << (bits - 1)) as f64;
            let mut r = reader;
            r.samples::<i32>()
                .map(|s| s.map(|v| v as f64 / full))
                .collect::<Result<_, _>>()
                .map_err(|e| ImportError::Malformed {
                    detail: e.to_string(),
                    offset: None,
                })?
        }
    };
    let n_frames = interleaved.len() / n_ch;
    if n_frames == 0 {
        return Err(ImportError::Malformed {
            detail: "no sample frames".into(),
            offset: None,
        });
    }
    let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or("wav");
    let t0 = cal.t0_epoch_s.map(|s| AbsTime {
        epoch_s: s,
        frac: 0.0,
    });
    let mut functions = Vec::new();
    for ch in 0..n_ch {
        let calc = cal.channels.get(ch);
        let (name, unit, scale) = match calc {
            Some(c) => (
                c.name.clone(),
                units::unit(&c.unit).unwrap_or_else(|| Unit::unknown(&c.unit)),
                c.eu_per_full_scale,
            ),
            None => {
                if !cal.channels.is_empty() {
                    warnings.push(format!("channel {} has no sidecar entry", ch + 1));
                }
                (
                    format!("{stem} channel {}", ch + 1),
                    Unit::unknown("FS"),
                    1.0,
                )
            }
        };
        let series: Vec<f64> = (0..n_frames)
            .map(|f| interleaved[f * n_ch + ch] * scale)
            .collect();
        functions.push(FunctionRecord {
            id: FnId(ch as u64 + 1),
            name,
            role: None,
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
                format: "wav".into(),
                path: path.display().to_string(),
                detail: format!(
                    "{} Hz, {} bit {}, {} ch",
                    spec.sample_rate,
                    spec.bits_per_sample,
                    match spec.sample_format {
                        hound::SampleFormat::Float => "float",
                        hound::SampleFormat::Int => "int",
                    },
                    n_ch
                ),
            },
            source: SourceRef {
                path: path.display().to_string(),
                format: "wav".into(),
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

/// Export TimeHistory records as 32-bit float WAV + the calibration sidecar
/// (strict: same fs required; the sidecar preserves names/units so a re-import
/// is lossless in meaning). Method notes ride the sidecar.
pub fn export(
    records: &[&FunctionRecord],
    path: &Path,
    method_note: Option<&str>,
) -> Result<(), String> {
    let first = records.first().ok_or("nothing to export")?;
    let Abscissa::Even { step, n, .. } = first.abscissa else {
        return Err("WAV export needs evenly-sampled time histories".into());
    };
    if step <= 0.0 {
        return Err("invalid sample step".into());
    }
    let fs = (1.0 / step).round();
    for r in records {
        if r.kind != FunctionKind::TimeHistory {
            return Err(format!("'{}' is not a time history", r.name));
        }
        match r.abscissa {
            Abscissa::Even {
                step: s2, n: n2, ..
            } => {
                if (s2 - step).abs() > step * 1e-9 || n2 != n {
                    return Err(format!("'{}' differs in rate/length", r.name));
                }
            }
            _ => return Err(format!("'{}' is not evenly sampled", r.name)),
        }
    }
    // Full-scale normalization: the loudest channel maps to 0.9 FS.
    let mut peak = 0.0f64;
    let series: Vec<&Vec<f64>> = records
        .iter()
        .map(|r| match &r.ordinate {
            Ordinate::Real(v) => {
                for x in v {
                    peak = peak.max(x.abs());
                }
                Ok(v)
            }
            Ordinate::Complex(_) => Err(format!("'{}' is complex", r.name)),
        })
        .collect::<Result<_, _>>()?;
    let eu_per_fs = if peak > 0.0 { peak / 0.9 } else { 1.0 };
    let spec = hound::WavSpec {
        channels: records.len() as u16,
        sample_rate: fs as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for f in 0..n {
        for s in &series {
            writer
                .write_sample((s[f] / eu_per_fs) as f32)
                .map_err(|e| e.to_string())?;
        }
    }
    writer.finalize().map_err(|e| e.to_string())?;
    let sidecar = CalSidecar {
        channels: records
            .iter()
            .map(|r| CalChannel {
                name: r.name.clone(),
                unit: r.unit_ordinate.label.clone(),
                eu_per_full_scale: eu_per_fs,
            })
            .collect(),
        t0_epoch_s: first.t0.map(|t| t.epoch_s),
    };
    let mut v = serde_json::to_value(&sidecar).map_err(|e| e.to_string())?;
    if let (Some(note), Some(obj)) = (method_note, v.as_object_mut()) {
        obj.insert("method".into(), serde_json::Value::String(note.into()));
    }
    std::fs::write(
        sidecar_path(path),
        serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use eigen_data::units::unit;

    fn rec(name: &str, vals: Vec<f64>, fs: f64) -> FunctionRecord {
        FunctionRecord {
            id: FnId(1),
            name: name.into(),
            role: None,
            response: None,
            reference: None,
            kind: FunctionKind::TimeHistory,
            abscissa: Abscissa::Even {
                start: 0.0,
                step: 1.0 / fs,
                n: vals.len(),
            },
            ordinate: Ordinate::Real(vals),
            unit_ordinate: unit("g").unwrap(),
            unit_abscissa: unit("s").unwrap(),
            t0: Some(AbsTime {
                epoch_s: 1_700_000_123,
                frac: 0.0,
            }),
            provenance: Provenance::Imported {
                format: "t".into(),
                path: "m".into(),
                detail: String::new(),
            },
            source: SourceRef {
                path: "m".into(),
                format: "t".into(),
                imported_at: 0,
            },
            comments: vec![],
        }
    }

    #[test]
    fn export_import_round_trip_preserves_eu_names_units_t0() {
        let dir = std::env::temp_dir().join("eigen_fmt_wav_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("run.wav");
        let a = rec(
            "head accel 101X+",
            (0..512).map(|i| 2.0 * (i as f64 * 0.1).sin()).collect(),
            2048.0,
        );
        let b = rec(
            "base ref 1Z+",
            (0..512).map(|i| -0.5 * (i as f64 * 0.07).cos()).collect(),
            2048.0,
        );
        export(&[&a, &b], &p, Some("insight test")).unwrap();
        let out = import(&p).unwrap();
        assert_eq!(out.functions.len(), 2);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_eq!(out.functions[0].name, "head accel 101X+");
        assert_eq!(out.functions[0].unit_ordinate.label, "g");
        assert_eq!(out.functions[0].t0.unwrap().epoch_s, 1_700_000_123);
        let (Ordinate::Real(orig), Ordinate::Real(back)) =
            (&a.ordinate, &out.functions[0].ordinate)
        else {
            panic!()
        };
        for (x, y) in orig.iter().zip(back) {
            assert!((x - y).abs() < 3e-6, "{x} vs {y} (f32 wav precision)");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn import_without_sidecar_warns_full_scale() {
        let dir = std::env::temp_dir().join("eigen_fmt_wav_test2");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("raw.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 1000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&p, spec).unwrap();
        for i in 0..64 {
            w.write_sample((i * 100) as i16 as i32).unwrap();
        }
        w.finalize().unwrap();
        let out = import(&p).unwrap();
        assert!(out.warnings.iter().any(|w| w.contains("FULL SCALE")));
        assert_eq!(out.functions[0].unit_ordinate.label, "FS");
        std::fs::remove_dir_all(dir).ok();
    }
}
