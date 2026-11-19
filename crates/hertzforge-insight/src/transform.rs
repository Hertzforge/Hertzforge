//! Time-domain transforms: filter / integrate / differentiate / resample /
//! cross-correlate. Every op produces NEW records with `Provenance::Derived`
//! and an exact recipe - sources are never mutated. Parameters validate HERE
//! so no kernel assert is reachable (the same rule as reprocess).

use hertzforge_data::{
    units, Abscissa, FnId, FunctionKind, FunctionRecord, Ordinate, Provenance, Quantity, SourceRef,
    Unit,
};
use hertzforge_dsp::{calculus, filter, resample, xcorr};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterShape {
    Lowpass,
    Highpass,
    /// HP(f1) x LP(f2) cascade - named exactly that in the recipe.
    Bandpass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum XcorrNormWire {
    Raw,
    Biased,
    Coeff,
}

/// One transform request. Filter/Integrate/Differentiate/Resample apply to
/// each selected time history independently; Xcorr takes exactly two.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransformOp {
    Filter {
        shape: FilterShape,
        /// Even order 2..=8 (per cascade leg for band-pass).
        order: usize,
        f1: f64,
        /// Band-pass upper edge; None otherwise.
        f2: Option<f64>,
        zero_phase: bool,
    },
    Integrate {
        /// Drift-guard high-pass in Hz; None disables (mean removal always on).
        drift_hp_hz: Option<f64>,
    },
    Differentiate,
    Resample {
        target_fs: f64,
    },
    Xcorr {
        norm: XcorrNormWire,
    },
}

#[derive(Debug)]
pub struct TransformOut {
    pub records: Vec<FunctionRecord>,
    pub warnings: Vec<String>,
}

fn time_series(rec: &FunctionRecord) -> Result<(&[f64], f64, f64), String> {
    if rec.kind != FunctionKind::TimeHistory {
        return Err(format!("'{}' is not a time history", rec.name));
    }
    let Abscissa::Even { start, step, .. } = rec.abscissa else {
        return Err(format!("'{}' is not evenly sampled", rec.name));
    };
    let Ordinate::Real(v) = &rec.ordinate else {
        return Err(format!("'{}' is complex", rec.name));
    };
    Ok((v, 1.0 / step, start))
}

// A private record constructor: the arguments are the derived record's fields and provenance.
#[allow(clippy::too_many_arguments)]
fn derived(
    source: &FunctionRecord,
    name: String,
    kind: FunctionKind,
    abscissa: Abscissa,
    ordinate: Ordinate,
    unit_ordinate: Unit,
    op: String,
    params_json: String,
    from: Vec<FnId>,
) -> FunctionRecord {
    FunctionRecord {
        id: FnId(0),
        name,
        role: None,
        response: source.response,
        reference: None,
        kind,
        abscissa,
        ordinate,
        unit_ordinate,
        unit_abscissa: units::unit("s").unwrap(),
        t0: source.t0,
        provenance: Provenance::Derived {
            op,
            params_json,
            from,
        },
        source: SourceRef {
            path: "session".into(),
            format: "derived".into(),
            imported_at: 0,
        },
        comments: vec![],
    }
}

/// Apply a per-record op (everything except Xcorr) to one source.
pub fn transform_one(source: &FunctionRecord, op: &TransformOp) -> Result<TransformOut, String> {
    let (x, fs, start) = time_series(source)?;
    let dt = 1.0 / fs;
    let mut warnings = Vec::new();
    let rec = match op {
        TransformOp::Filter {
            shape,
            order,
            f1,
            f2,
            zero_phase,
        } => {
            let kind = match shape {
                FilterShape::Lowpass => filter::FilterKind::Lowpass,
                FilterShape::Highpass => filter::FilterKind::Highpass,
                FilterShape::Bandpass => filter::FilterKind::Bandpass,
            };
            let sections = filter::design_butterworth(kind, *order, *f1, *f2, fs)?;
            let y = if *zero_phase {
                filter::filtfilt(&sections, x)
            } else {
                filter::filter(&sections, x)
            };
            let (tag, opline) = match shape {
                FilterShape::Lowpass => ("LP", format!("Butterworth LP {order} @ {f1} Hz")),
                FilterShape::Highpass => ("HP", format!("Butterworth HP {order} @ {f1} Hz")),
                FilterShape::Bandpass => (
                    "BP",
                    format!(
                        "Butterworth BP (HP {f1} x LP {} cascade, {order}+{order})",
                        f2.unwrap_or(0.0)
                    ),
                ),
            };
            let opline = if *zero_phase {
                format!("{opline}, zero-phase (fwd-bwd, |H|^2)")
            } else {
                opline
            };
            derived(
                source,
                format!("{} · {tag} {f1} Hz", source.name),
                FunctionKind::TimeHistory,
                source.abscissa.clone(),
                Ordinate::Real(y),
                source.unit_ordinate.clone(),
                opline,
                serde_json::json!({
                    "shape": shape, "order": order, "f1": f1, "f2": f2,
                    "zero_phase": zero_phase
                })
                .to_string(),
                vec![source.id],
            )
        }
        TransformOp::Integrate { drift_hp_hz } => {
            let y = calculus::integrate(x, dt, *drift_hp_hz)?;
            let u = &source.unit_ordinate;
            let unit = Unit::custom(&format!("{}·s", u.label), Quantity::Unknown, u.si_factor);
            let guard = match drift_hp_hz {
                Some(f) => format!("drift HP {f} Hz"),
                None => "no drift HP".into(),
            };
            derived(
                source,
                format!("∫ {}", source.name),
                FunctionKind::TimeHistory,
                source.abscissa.clone(),
                Ordinate::Real(y),
                unit,
                format!(
                    "integrate (trapezoid, mean removed, {guard}; symbolic units - no gravitational conversion)"
                ),
                serde_json::json!({ "hp_hz": drift_hp_hz }).to_string(),
                vec![source.id],
            )
        }
        TransformOp::Differentiate => {
            let y = calculus::differentiate(x, dt)?;
            let u = &source.unit_ordinate;
            let unit = Unit::custom(&format!("{}/s", u.label), Quantity::Unknown, u.si_factor);
            derived(
                source,
                format!("d/dt {}", source.name),
                FunctionKind::TimeHistory,
                source.abscissa.clone(),
                Ordinate::Real(y),
                unit,
                "differentiate (central difference; symbolic units)".into(),
                "{}".into(),
                vec![source.id],
            )
        }
        TransformOp::Resample { target_fs } => {
            let r = resample::resample(x, fs, *target_fs)?;
            if (r.fs_out - target_fs).abs() > 1e-6 * target_fs {
                warnings.push(format!(
                    "requested {target_fs} Hz is not rational within L/M <= {}; achieved {} Hz",
                    resample::MAX_FACTOR,
                    r.fs_out
                ));
            }
            if r.fs_out < fs {
                warnings.push(format!(
                    "downsampling: content above {:.1} Hz removed by the anti-alias filter",
                    0.45 * r.fs_out
                ));
            }
            let n = r.y.len();
            derived(
                source,
                format!("{} @ {:.6} Hz", source.name, r.fs_out),
                FunctionKind::TimeHistory,
                Abscissa::Even {
                    start,
                    step: 1.0 / r.fs_out,
                    n,
                },
                Ordinate::Real(r.y),
                source.unit_ordinate.clone(),
                format!(
                    "resample {fs} -> {} Hz (L/M {}/{}, polyphase sinc, Kaiser ~80 dB)",
                    r.fs_out, r.l, r.m
                ),
                serde_json::json!({
                    "requested_fs": target_fs, "achieved_fs": r.fs_out,
                    "l": r.l, "m": r.m
                })
                .to_string(),
                vec![source.id],
            )
        }
        TransformOp::Xcorr { .. } => {
            return Err("cross-correlation takes exactly two functions".into())
        }
    };
    Ok(TransformOut {
        records: vec![rec],
        warnings,
    })
}

/// Cross-correlate b against a (selection order [a, b]).
pub fn transform_xcorr(
    a: &FunctionRecord,
    b: &FunctionRecord,
    norm: XcorrNormWire,
) -> Result<TransformOut, String> {
    let (xa, fsa, _) = time_series(a)?;
    let (xb, fsb, _) = time_series(b)?;
    if ((fsa - fsb) / fsa).abs() > 1e-9 {
        return Err(format!(
            "sample rates differ ({fsa} vs {fsb} Hz) - resample one first (Transform -> Resample)"
        ));
    }
    let n = match norm {
        XcorrNormWire::Raw => xcorr::XcorrNorm::Raw,
        XcorrNormWire::Biased => xcorr::XcorrNorm::Biased,
        XcorrNormWire::Coeff => xcorr::XcorrNorm::Coeff,
    };
    let r = xcorr::xcorr(xa, xb, n)?;
    let dt = 1.0 / fsa;
    let unit = match norm {
        XcorrNormWire::Coeff => Unit::custom("", Quantity::Dimensionless, 1.0),
        _ => Unit::custom(
            &format!("{}·{}", a.unit_ordinate.label, b.unit_ordinate.label),
            Quantity::Unknown,
            a.unit_ordinate.si_factor * b.unit_ordinate.si_factor,
        ),
    };
    let norm_label = match norm {
        XcorrNormWire::Raw => "raw",
        XcorrNormWire::Biased => "biased",
        XcorrNormWire::Coeff => "coeff",
    };
    let n_out = r.values.len();
    let rec = FunctionRecord {
        id: FnId(0),
        name: format!("xcorr: {} vs {}", b.name, a.name),
        role: None,
        response: b.response,
        reference: a.response,
        kind: FunctionKind::Correlation,
        abscissa: Abscissa::Even {
            start: r.first_lag as f64 * dt,
            step: dt,
            n: n_out,
        },
        ordinate: Ordinate::Real(r.values),
        unit_ordinate: unit,
        unit_abscissa: units::unit("s").unwrap(),
        t0: None,
        provenance: Provenance::Derived {
            op: format!(
                "xcorr '{}' vs '{}' ({norm_label}); positive lag: '{}' lags '{}'",
                b.name, a.name, b.name, a.name
            ),
            params_json: serde_json::json!({ "norm": norm_label }).to_string(),
            from: vec![a.id, b.id],
        },
        source: SourceRef {
            path: "session".into(),
            format: "derived".into(),
            imported_at: 0,
        },
        comments: vec![],
    };
    Ok(TransformOut {
        records: vec![rec],
        warnings: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::TAU;

    const FS: f64 = 2048.0;

    fn th(name: &str, y: Vec<f64>) -> FunctionRecord {
        FunctionRecord {
            id: FnId(7),
            name: name.into(),
            role: None,
            response: None,
            reference: None,
            kind: FunctionKind::TimeHistory,
            abscissa: Abscissa::Even {
                start: 0.0,
                step: 1.0 / FS,
                n: y.len(),
            },
            ordinate: Ordinate::Real(y),
            unit_ordinate: units::unit("g").unwrap(),
            unit_abscissa: units::unit("s").unwrap(),
            t0: None,
            provenance: Provenance::Imported {
                format: "test".into(),
                path: "t".into(),
                detail: String::new(),
            },
            source: SourceRef {
                path: "t".into(),
                format: "test".into(),
                imported_at: 0,
            },
            comments: vec![],
        }
    }

    fn two_tone() -> FunctionRecord {
        th(
            "mix",
            (0..8192)
                .map(|i| {
                    let t = i as f64 / FS;
                    (TAU * 50.0 * t).sin() + (TAU * 700.0 * t).sin()
                })
                .collect(),
        )
    }

    fn rms(v: &[f64]) -> f64 {
        let s = &v[v.len() / 8..7 * v.len() / 8];
        (s.iter().map(|a| a * a).sum::<f64>() / s.len() as f64).sqrt()
    }

    #[test]
    fn lowpass_removes_the_high_tone_and_stamps_the_recipe() {
        let src = two_tone();
        let out = transform_one(
            &src,
            &TransformOp::Filter {
                shape: FilterShape::Lowpass,
                order: 6,
                f1: 200.0,
                f2: None,
                zero_phase: true,
            },
        )
        .unwrap();
        let rec = &out.records[0];
        let Ordinate::Real(y) = &rec.ordinate else {
            panic!()
        };
        // Two equal tones: rms sqrt(2)*0.707 = 1.0 in; ~0.707 out (one tone).
        assert!((rms(y) - 0.707).abs() < 0.02, "{}", rms(y));
        let recipe = rec.provenance.recipe_line();
        assert!(recipe.contains("Butterworth LP 6 @ 200 Hz"), "{recipe}");
        assert!(recipe.contains("zero-phase"), "{recipe}");
        assert_eq!(rec.unit_ordinate.label, "g");
        assert_eq!(rec.kind, FunctionKind::TimeHistory);
    }

    #[test]
    fn integrate_gets_symbolic_units_and_the_no_conversion_note() {
        let src = two_tone();
        let out = transform_one(
            &src,
            &TransformOp::Integrate {
                drift_hp_hz: Some(2.0),
            },
        )
        .unwrap();
        let rec = &out.records[0];
        assert_eq!(rec.unit_ordinate.label, "g·s");
        assert!(rec
            .provenance
            .recipe_line()
            .contains("no gravitational conversion"));
        let out2 = transform_one(&src, &TransformOp::Differentiate).unwrap();
        assert_eq!(out2.records[0].unit_ordinate.label, "g/s");
    }

    #[test]
    fn resample_stamps_the_achieved_rate_and_discloses_downsampling() {
        let src = two_tone();
        let out = transform_one(&src, &TransformOp::Resample { target_fs: 1024.0 }).unwrap();
        let rec = &out.records[0];
        let Abscissa::Even { step, .. } = rec.abscissa else {
            panic!()
        };
        assert!((1.0 / step - 1024.0).abs() < 1e-9);
        assert!(
            rec.provenance.recipe_line().contains("2048 -> 1024 Hz"),
            "{}",
            rec.provenance.recipe_line()
        );
        assert!(
            out.warnings.iter().any(|w| w.contains("anti-alias")),
            "{:?}",
            out.warnings
        );
        // The 700 Hz tone sat above the new usable band and is gone.
        let Ordinate::Real(y) = &rec.ordinate else {
            panic!()
        };
        assert!((rms(y) - 0.707).abs() < 0.03, "{}", rms(y));
    }

    #[test]
    fn xcorr_produces_a_correlation_record_with_the_pinned_convention() {
        let a = two_tone();
        let d = 25usize;
        let Ordinate::Real(av) = &a.ordinate else {
            panic!()
        };
        let mut bv = vec![0.0; av.len()];
        bv[d..].copy_from_slice(&av[..av.len() - d]);
        let b = th("delayed", bv);
        let out = transform_xcorr(&a, &b, XcorrNormWire::Coeff).unwrap();
        let rec = &out.records[0];
        assert_eq!(rec.kind, FunctionKind::Correlation);
        assert_eq!(rec.unit_abscissa.label, "s");
        let Ordinate::Real(v) = &rec.ordinate else {
            panic!()
        };
        let Abscissa::Even { start, step, .. } = rec.abscissa else {
            panic!()
        };
        let (imax, _) = v
            .iter()
            .enumerate()
            .max_by(|x, y| x.1.partial_cmp(y.1).unwrap())
            .unwrap();
        let lag_s = start + imax as f64 * step;
        assert!((lag_s - d as f64 / FS).abs() < 1e-9, "peak at {lag_s}");
        assert!(
            rec.provenance.recipe_line().contains("positive lag"),
            "{}",
            rec.provenance.recipe_line()
        );
    }

    #[test]
    fn refusals_are_clean() {
        let a = two_tone();
        let mut b = two_tone();
        b.abscissa = Abscissa::Even {
            start: 0.0,
            step: 1.0 / 4096.0,
            n: 8192,
        };
        let e = transform_xcorr(&a, &b, XcorrNormWire::Coeff).unwrap_err();
        assert!(e.contains("Transform -> Resample"), "{e}");

        let mut not_time = two_tone();
        not_time.kind = FunctionKind::Psd;
        assert!(transform_one(&not_time, &TransformOp::Differentiate).is_err());
        assert!(transform_one(
            &a,
            &TransformOp::Xcorr {
                norm: XcorrNormWire::Raw
            }
        )
        .is_err());
        assert!(transform_one(
            &a,
            &TransformOp::Filter {
                shape: FilterShape::Lowpass,
                order: 5,
                f1: 100.0,
                f2: None,
                zero_phase: false
            }
        )
        .is_err());
    }
}
